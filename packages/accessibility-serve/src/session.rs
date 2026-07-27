//! Ownership of the per-device simulator resources.
//!
//! The simulator's Objective-C objects are not `Sync` and their calls block, so
//! each one lives on a dedicated thread behind a command channel rather than in
//! a mutex on the async runtime.
//!
//! Input and accessibility get *separate* threads on purpose: an accessibility
//! tree fetch can take hundreds of milliseconds, and pointer events queued
//! behind one would make the stream feel broken.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Result, anyhow};
use tokio::sync::{broadcast, mpsc, oneshot};

use accessibility_core::video::{EncodedFrame, FrameKind, VideoCapture, VideoConfig};

use crate::ax::{AxCommand, AxSnapshot, ElementDetail, spawn_ax_worker};
use crate::input::{InputCommand, spawn_input_worker};

/// How many encoded frames to buffer per subscriber.
///
/// Small on purpose: for interactive video a dropped frame is better than a
/// late one, and a slow client should not be able to inflate memory.
const FRAME_BUFFER: usize = 16;

/// Geometry and identity of the device being served.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeviceInfo {
    pub udid: String,
    pub width: u32,
    pub height: u32,
}

pub struct SimSession {
    device_udid: String,
    capture: Box<dyn VideoCapture>,
    frames: broadcast::Sender<EncodedFrame>,
    /// Most recent parameter set, replayed to clients that join mid-stream.
    latest_parameter_set: Arc<std::sync::Mutex<Option<EncodedFrame>>>,
    frames_encoded: Arc<AtomicU64>,
    input: mpsc::UnboundedSender<InputCommand>,
    ax: mpsc::UnboundedSender<AxCommand>,
}

impl SimSession {
    /// Attach to a booted simulator and start capturing.
    pub fn start(udid: Option<&str>, config: VideoConfig) -> Result<Arc<Self>> {
        let (frames, _) = broadcast::channel(FRAME_BUFFER);
        let latest_parameter_set = Arc::new(std::sync::Mutex::new(None));
        let frames_encoded = Arc::new(AtomicU64::new(0));

        let sink = {
            let frames = frames.clone();
            let latest_parameter_set = Arc::clone(&latest_parameter_set);
            let frames_encoded = Arc::clone(&frames_encoded);
            Arc::new(move |frame: EncodedFrame| {
                if frame.kind == FrameKind::ParameterSet {
                    *latest_parameter_set.lock().unwrap() = Some(frame.clone());
                }
                frames_encoded.fetch_add(1, Ordering::Relaxed);
                // A send error just means nobody is watching yet.
                let _ = frames.send(frame);
            })
        };

        let (capture, resolved_udid) = start_capture(udid, &config, sink)?;
        let input = spawn_input_worker(&resolved_udid)?;
        let ax = spawn_ax_worker(&resolved_udid)?;

        Ok(Arc::new(Self {
            device_udid: resolved_udid,
            capture,
            frames,
            latest_parameter_set,
            frames_encoded,
            input,
            ax,
        }))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EncodedFrame> {
        let receiver = self.frames.subscribe();
        // A new subscriber cannot decode anything until the next keyframe, so
        // ask for one immediately instead of making them wait out the interval.
        self.capture.request_keyframe();
        receiver
    }

    pub fn latest_parameter_set(&self) -> Option<EncodedFrame> {
        self.latest_parameter_set.lock().unwrap().clone()
    }

    pub fn request_keyframe(&self) {
        self.capture.request_keyframe();
    }

    pub fn frames_encoded(&self) -> u64 {
        self.frames_encoded.load(Ordering::Relaxed)
    }

    pub fn device_info(&self) -> DeviceInfo {
        let geometry = self.capture.geometry();
        DeviceInfo {
            udid: self.device_udid.clone(),
            width: geometry.width,
            height: geometry.height,
        }
    }

    /// Queue an input event. Fire-and-forget: pointer events must never block
    /// the socket reader.
    pub fn send_input(&self, command: InputCommand) {
        let _ = self.input.send(command);
    }

    pub async fn ax_snapshot(&self) -> Result<AxSnapshot> {
        let (tx, rx) = oneshot::channel();
        self.ax
            .send(AxCommand::Snapshot { reply: tx })
            .map_err(|_| anyhow!("accessibility worker stopped"))?;
        rx.await.map_err(|_| anyhow!("accessibility worker stopped"))?
    }

    pub async fn ax_hit_test(&self, x: f64, y: f64) -> Result<Option<ElementDetail>> {
        let (tx, rx) = oneshot::channel();
        self.ax
            .send(AxCommand::HitTest { x, y, reply: tx })
            .map_err(|_| anyhow!("accessibility worker stopped"))?;
        rx.await.map_err(|_| anyhow!("accessibility worker stopped"))?
    }
}

#[cfg(target_os = "macos")]
fn start_capture(
    udid: Option<&str>,
    config: &VideoConfig,
    sink: accessibility_core::video::FrameSink,
) -> Result<(Box<dyn VideoCapture>, String)> {
    use accessibility_core::platform::ios_simulator::SimulatorVideoCapture;

    // Resolve the concrete UDID up front so the input and accessibility
    // workers bind to the same device the video came from, even when the
    // caller passed `None`.
    let resolved = accessibility_ios_sys::SimFramebuffer::new(udid)?
        .device_udid()
        .to_string();
    let capture = SimulatorVideoCapture::start(&resolved, config, sink)?;
    Ok((Box::new(capture), resolved))
}

#[cfg(not(target_os = "macos"))]
fn start_capture(
    _udid: Option<&str>,
    _config: &VideoConfig,
    _sink: accessibility_core::video::FrameSink,
) -> Result<(Box<dyn VideoCapture>, String)> {
    anyhow::bail!("Serving an iOS Simulator requires macOS")
}
