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
use crate::input::{InputCommand, Orientation, spawn_input_worker};
use crate::settings::{Setting, SettingKey};

/// How many encoded frames to buffer per subscriber.
///
/// Small on purpose: for interactive video a dropped frame is better than a
/// late one, and a slow client should not be able to inflate memory.
const FRAME_BUFFER: usize = 16;

/// Geometry and identity of the device being served.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeviceInfo {
    pub udid: String,
    /// Raw framebuffer width. Does not change with orientation.
    pub width: u32,
    /// Raw framebuffer height. Does not change with orientation.
    pub height: u32,
    pub orientation: Orientation,
}

pub struct SimSession {
    device_udid: String,
    capture: Box<dyn VideoCapture>,
    frames: broadcast::Sender<EncodedFrame>,
    /// Most recent parameter set, replayed to clients that join mid-stream.
    latest_parameter_set: Arc<std::sync::Mutex<Option<EncodedFrame>>>,
    frames_encoded: Arc<AtomicU64>,
    input: std::sync::mpsc::Sender<InputCommand>,
    ax: mpsc::UnboundedSender<AxCommand>,
    /// Last orientation we asked for.
    ///
    /// The framebuffer is always portrait-native — rotating the device
    /// rotates the *content* inside a fixed-size surface — so orientation
    /// cannot be recovered from the video and has to be tracked here.
    orientation: std::sync::Mutex<Orientation>,
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
            orientation: std::sync::Mutex::new(Orientation::Portrait),
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
            orientation: self.orientation(),
        }
    }

    pub fn orientation(&self) -> Orientation {
        *self.orientation.lock().unwrap()
    }

    /// Rotate the device and remember the new orientation.
    pub fn set_orientation(&self, orientation: Orientation) {
        *self.orientation.lock().unwrap() = orientation;
        self.send_input(InputCommand::Rotate { orientation });
    }

    pub fn settings(&self) -> Vec<Setting> {
        crate::settings::read_all(&self.device_udid)
    }

    pub fn set_setting(&self, key: SettingKey, value: &str) -> Result<String> {
        crate::settings::write(&self.device_udid, key, value)
    }

    /// Queue an input event. Fire-and-forget: pointer events must never block
    /// the socket reader.
    pub fn send_input(&self, command: InputCommand) {
        if let InputCommand::Rotate { orientation } = command {
            *self.orientation.lock().unwrap() = orientation;
        }
        let _ = self.input.send(command);
    }

    pub async fn ax_snapshot(&self) -> Result<AxSnapshot> {
        let (tx, rx) = oneshot::channel();
        self.ax
            .send(AxCommand::Snapshot { reply: tx })
            .map_err(|_| anyhow!("accessibility worker stopped"))?;
        let snapshot = rx
            .await
            .map_err(|_| anyhow!("accessibility worker stopped"))??;
        self.reconcile_orientation(snapshot.is_landscape);
        Ok(snapshot)
    }

    /// Correct the tracked orientation against what the device reports.
    ///
    /// Orientation can change without us: the user can rotate from the
    /// Simulator menu, an app can force an orientation, or the server can be
    /// restarted while the device is already sideways. Accessibility bounds
    /// are the only cheap signal, and they only reveal landscape vs portrait,
    /// so a disagreement resolves to a sensible default of the right kind
    /// rather than to an exact rotation.
    fn reconcile_orientation(&self, is_landscape: bool) {
        let mut orientation = self.orientation.lock().unwrap();
        if orientation.is_landscape() == is_landscape {
            return;
        }
        *orientation = if is_landscape {
            Orientation::LandscapeLeft
        } else {
            Orientation::Portrait
        };
    }

    /// Best-effort orientation seed at startup.
    ///
    /// Without this the server would assume portrait and render a sideways
    /// device whenever it attaches to an already-rotated simulator.
    pub async fn seed_orientation(&self) {
        if let Ok(snapshot) = self.ax_snapshot().await {
            self.reconcile_orientation(snapshot.is_landscape);
        }
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
