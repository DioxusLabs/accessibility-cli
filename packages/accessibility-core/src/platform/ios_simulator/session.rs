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
use std::time::Instant;

use anyhow::{Result, anyhow};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::video::{
    EncodedFrame, FrameKind, Recording, RecordingConfig, VideoCapture, VideoConfig,
};

use super::SimulatorVideoCapture;
use super::ax::{AxCommand, AxSnapshot, ElementDetail, spawn_ax_worker};
use super::input::{
    InputCapabilities, InputCommand, Orientation, spawn_input_worker_with_capabilities,
};
use super::settings::{self, Setting, SettingKey};

/// How many encoded frames to buffer per subscriber.
///
/// Small on purpose: for interactive video a dropped frame is better than a
/// late one, and a slow client should not be able to inflate memory.
const FRAME_BUFFER: usize = 16;

/// Counters for diagnosing stream quality and pacing.
///
/// Cheap enough to always collect: the interesting failures here (keyframe
/// storms, subscribers falling behind) are invisible without them and only
/// show up under load, which is exactly when you cannot attach a profiler.
#[derive(Default)]
pub struct StreamStats {
    pub frames: AtomicU64,
    pub keyframes: AtomicU64,
    pub bytes: AtomicU64,
    /// Keyframes asked for by a new subscriber, an RTCP PLI, or a lagging
    /// receiver. A high rate here starves the stream of bitrate for delta
    /// frames and is self-reinforcing.
    pub keyframe_requests: AtomicU64,
    /// Times a subscriber fell far enough behind to drop frames.
    pub lag_events: AtomicU64,
}

/// A snapshot of [`StreamStats`] with rates worked out.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StatsReport {
    pub uptime_secs: f64,
    pub frames: u64,
    pub keyframes: u64,
    pub bytes: u64,
    pub fps: f64,
    pub mbps: f64,
    pub bits_per_pixel: f64,
    pub mean_frame_kb: f64,
    pub keyframe_requests: u64,
    pub lag_events: u64,
    pub subscribers: usize,
    /// Frames written to the current recording, or `None` when idle.
    pub recording_frames: Option<u64>,
    /// Capture resolution.
    pub width: u32,
    pub height: u32,
    /// Resolution actually encoded, after downscaling.
    pub encoded_width: u32,
    pub encoded_height: u32,
}

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

/// A transport-neutral, hardware-encoded iOS Simulator session.
pub struct SimSession {
    device_udid: String,
    capture: Box<dyn VideoCapture>,
    frames: broadcast::Sender<EncodedFrame>,
    /// Most recent parameter set, replayed to clients that join mid-stream.
    latest_parameter_set: Arc<std::sync::Mutex<Option<EncodedFrame>>>,
    stats: Arc<StreamStats>,
    started: Instant,
    input: std::sync::mpsc::Sender<InputCommand>,
    input_capabilities: InputCapabilities,
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
        let stats = Arc::new(StreamStats::default());

        let sink = {
            let frames = frames.clone();
            let latest_parameter_set = Arc::clone(&latest_parameter_set);
            let stats = Arc::clone(&stats);
            Arc::new(move |frame: EncodedFrame| {
                if frame.kind == FrameKind::ParameterSet {
                    *latest_parameter_set.lock().unwrap() = Some(frame.clone());
                }
                stats.frames.fetch_add(1, Ordering::Relaxed);
                stats
                    .bytes
                    .fetch_add(frame.data.len() as u64, Ordering::Relaxed);
                if frame.kind == FrameKind::Keyframe {
                    stats.keyframes.fetch_add(1, Ordering::Relaxed);
                }
                // A send error just means nobody is watching yet.
                let _ = frames.send(frame);
            })
        };

        let (capture, resolved_udid) = start_capture(udid, &config, sink)?;
        let (input, input_capabilities) = spawn_input_worker_with_capabilities(&resolved_udid)?;
        let ax = spawn_ax_worker(&resolved_udid)?;

        Ok(Arc::new(Self {
            device_udid: resolved_udid,
            capture,
            frames,
            latest_parameter_set,
            stats,
            started: Instant::now(),
            input,
            input_capabilities,
            ax,
            orientation: std::sync::Mutex::new(Orientation::Portrait),
        }))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EncodedFrame> {
        let receiver = self.frames.subscribe();
        #[allow(clippy::let_and_return)]
        // A new subscriber cannot decode anything until the next keyframe, so
        // ask for one immediately instead of making them wait out the interval.
        self.capture.request_keyframe();
        receiver
    }

    pub fn latest_parameter_set(&self) -> Option<EncodedFrame> {
        self.latest_parameter_set.lock().unwrap().clone()
    }

    pub fn request_keyframe(&self) {
        self.stats.keyframe_requests.fetch_add(1, Ordering::Relaxed);
        self.capture.request_keyframe();
    }

    pub fn note_lag(&self) {
        self.stats.lag_events.fetch_add(1, Ordering::Relaxed);
    }

    pub fn stats(&self) -> StatsReport {
        let elapsed = self.started.elapsed().as_secs_f64().max(1e-6);
        let frames = self.stats.frames.load(Ordering::Relaxed);
        let bytes = self.stats.bytes.load(Ordering::Relaxed);
        let geometry = self.capture.geometry();
        let encoded = self.capture.encoded_geometry();
        // Bits per pixel only means anything against the encoded size.
        let pixels = (encoded.width as f64) * (encoded.height as f64);
        let fps = frames as f64 / elapsed;

        StatsReport {
            uptime_secs: (elapsed * 10.0).round() / 10.0,
            frames,
            keyframes: self.stats.keyframes.load(Ordering::Relaxed),
            bytes,
            fps: (fps * 10.0).round() / 10.0,
            mbps: ((bytes as f64 * 8.0 / elapsed / 1e6) * 100.0).round() / 100.0,
            // The headline number: anything much under 0.1 will visibly
            // block up on motion.
            bits_per_pixel: if pixels > 0.0 && fps > 0.0 {
                ((bytes as f64 * 8.0 / elapsed) / (pixels * fps) * 10000.0).round() / 10000.0
            } else {
                0.0
            },
            mean_frame_kb: if frames > 0 {
                ((bytes as f64 / frames as f64 / 1024.0) * 100.0).round() / 100.0
            } else {
                0.0
            },
            keyframe_requests: self.stats.keyframe_requests.load(Ordering::Relaxed),
            lag_events: self.stats.lag_events.load(Ordering::Relaxed),
            subscribers: self.frames.receiver_count(),
            recording_frames: self.capture.recording_frames(),
            width: geometry.width,
            height: geometry.height,
            encoded_width: encoded.width,
            encoded_height: encoded.height,
        }
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

    pub fn input_capabilities(&self) -> InputCapabilities {
        self.input_capabilities
    }

    pub fn orientation(&self) -> Orientation {
        *self.orientation.lock().unwrap()
    }

    /// Rotate the device and remember the new orientation.
    pub fn set_orientation(&self, orientation: Orientation) {
        *self.orientation.lock().unwrap() = orientation;
        self.send_input(InputCommand::Rotate { orientation });
    }

    /// Begin recording to a file beside the system temp directory.
    ///
    /// Runs a second encode of the same frames, so the live stream is
    /// unaffected and the recording can use B-frames and its own resolution.
    pub fn start_recording(&self, config: RecordingConfig) -> Result<std::path::PathBuf> {
        let path = std::env::temp_dir().join(format!(
            "serve-sim-{}-{}.mp4",
            self.device_udid,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default()
        ));
        self.capture.start_recording(&path, &config)?;
        Ok(path)
    }

    pub fn stop_recording(&self) -> Result<Recording> {
        self.capture.stop_recording()
    }

    pub fn recording_frames(&self) -> Option<u64> {
        self.capture.recording_frames()
    }

    pub fn settings(&self) -> Vec<Setting> {
        settings::read_all(&self.device_udid)
    }

    pub fn set_setting(&self, key: SettingKey, value: &str) -> Result<String> {
        settings::write(&self.device_udid, key, value)
    }

    /// Queue an input event. Fire-and-forget: pointer events must never block
    /// the socket reader.
    pub fn send_input(&self, command: InputCommand) {
        if let InputCommand::Rotate { orientation } = command {
            *self.orientation.lock().unwrap() = orientation;
        }
        let _ = self.input.send(command);
    }

    /// Read the accessibility tree.
    ///
    /// `scan` additionally hit-tests the regions the tree walk cannot explain,
    /// which is the only way to reach `WKWebView` and Safari content. It costs
    /// a few hundred milliseconds, so it is opt-in.
    pub async fn ax_snapshot(&self, scan: bool) -> Result<AxSnapshot> {
        let (tx, rx) = oneshot::channel();
        self.ax
            .send(AxCommand::Snapshot { scan, reply: tx })
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
        if let Ok(snapshot) = self.ax_snapshot(false).await {
            self.reconcile_orientation(snapshot.is_landscape);
        }
    }

    pub async fn ax_hit_test(&self, x: f64, y: f64) -> Result<Option<ElementDetail>> {
        let (tx, rx) = oneshot::channel();
        self.ax
            .send(AxCommand::HitTest { x, y, reply: tx })
            .map_err(|_| anyhow!("accessibility worker stopped"))?;
        rx.await
            .map_err(|_| anyhow!("accessibility worker stopped"))?
    }
}

fn start_capture(
    udid: Option<&str>,
    config: &VideoConfig,
    sink: crate::video::FrameSink,
) -> Result<(Box<dyn VideoCapture>, String)> {
    // Resolve the concrete UDID up front so the input and accessibility
    // workers bind to the same device the video came from, even when the
    // caller passed `None`.
    let resolved = accessibility_ios_sys::SimFramebuffer::new(udid)?
        .device_udid()
        .to_string();
    let capture = SimulatorVideoCapture::start(&resolved, config, sink)?;
    Ok((Box::new(capture), resolved))
}
