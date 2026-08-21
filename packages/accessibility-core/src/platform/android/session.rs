use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use anyhow::{Result, anyhow, bail};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::video::{EncodedFrame, FrameKind, ScreenGeometry, VideoCapture, VideoConfig};

use super::ax::{AxCommand, AxSnapshot, ElementDetail, spawn_ax_worker};
use super::input::{InputCommand, Orientation, set_device_orientation, spawn_input_worker};
use super::{AdbClient, AndroidVideoCapture};

const FRAME_BUFFER: usize = 16;

#[derive(Default)]
pub struct StreamStats {
    pub frames: AtomicU64,
    pub keyframes: AtomicU64,
    pub bytes: AtomicU64,
    pub keyframe_requests: AtomicU64,
    pub lag_events: AtomicU64,
}

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
    pub recording_frames: Option<u64>,
    pub width: u32,
    pub height: u32,
    pub encoded_width: u32,
    pub encoded_height: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DeviceInfo {
    pub serial: String,
    pub width: u32,
    pub height: u32,
    pub orientation: Orientation,
}

pub struct EmulatorSession {
    serial: String,
    adb: AdbClient,
    capture: AndroidVideoCapture,
    frames: broadcast::Sender<EncodedFrame>,
    stats: Arc<StreamStats>,
    started: Instant,
    input: tokio::sync::mpsc::UnboundedSender<InputCommand>,
    ax: mpsc::UnboundedSender<AxCommand>,
    orientation: std::sync::Mutex<Orientation>,
}

impl EmulatorSession {
    pub async fn start(serial: Option<&str>, config: VideoConfig) -> Result<Arc<Self>> {
        let adb = AdbClient::discover(serial);
        let serial = adb.resolved_serial().await?;
        if !serial.starts_with("emulator-") {
            bail!("Android Emulator streaming requires an emulator serial, got '{serial}'");
        }
        let adb = AdbClient::discover(Some(&serial));
        let (width, height) = adb.get_screen_size().await?;
        let geometry = ScreenGeometry { width, height };
        let (frames, _) = broadcast::channel(FRAME_BUFFER);
        let stats = Arc::new(StreamStats::default());
        let sink = {
            let frames = frames.clone();
            let stats = Arc::clone(&stats);
            Arc::new(move |frame: EncodedFrame| {
                stats.frames.fetch_add(1, Ordering::Relaxed);
                stats
                    .bytes
                    .fetch_add(frame.data.len() as u64, Ordering::Relaxed);
                if frame.kind == FrameKind::Keyframe {
                    stats.keyframes.fetch_add(1, Ordering::Relaxed);
                }
                let _ = frames.send(frame);
            })
        };
        let capture = AndroidVideoCapture::start(adb.clone(), geometry, &config, sink)?;
        let input = spawn_input_worker(&serial, geometry).await?;
        let ax = spawn_ax_worker(&serial).await?;
        Ok(Arc::new(Self {
            serial,
            adb,
            capture,
            frames,
            stats,
            started: Instant::now(),
            input,
            ax,
            orientation: std::sync::Mutex::new(Orientation::Portrait),
        }))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EncodedFrame> {
        let receiver = self.frames.subscribe();
        self.capture.request_keyframe();
        receiver
    }

    pub fn request_keyframe(&self) {
        self.stats.keyframe_requests.fetch_add(1, Ordering::Relaxed);
        self.capture.request_keyframe();
    }

    pub fn note_lag(&self) {
        self.stats.lag_events.fetch_add(1, Ordering::Relaxed);
    }

    pub fn device_info(&self) -> DeviceInfo {
        let geometry = self.capture.geometry();
        DeviceInfo {
            serial: self.serial.clone(),
            width: geometry.width,
            height: geometry.height,
            orientation: self.orientation(),
        }
    }

    pub fn orientation(&self) -> Orientation {
        *self.orientation.lock().unwrap()
    }

    pub async fn set_orientation(&self, orientation: Orientation) -> Result<()> {
        set_device_orientation(&self.adb, orientation).await?;
        self.capture.set_landscape(orientation.is_landscape())?;
        *self.orientation.lock().unwrap() = orientation;
        Ok(())
    }

    pub async fn send_input(&self, command: InputCommand) {
        if let InputCommand::Rotate { orientation } = command {
            let _ = self.set_orientation(orientation).await;
            return;
        }
        let _ = self.input.send(command);
    }

    pub fn stats(&self) -> StatsReport {
        let elapsed = self.started.elapsed().as_secs_f64().max(1e-6);
        let frames = self.stats.frames.load(Ordering::Relaxed);
        let bytes = self.stats.bytes.load(Ordering::Relaxed);
        let geometry = self.capture.geometry();
        let encoded = self.capture.encoded_geometry();
        let pixels = encoded.width as f64 * encoded.height as f64;
        let fps = frames as f64 / elapsed;
        StatsReport {
            uptime_secs: (elapsed * 10.0).round() / 10.0,
            frames,
            keyframes: self.stats.keyframes.load(Ordering::Relaxed),
            bytes,
            fps: (fps * 10.0).round() / 10.0,
            mbps: ((bytes as f64 * 8.0 / elapsed / 1e6) * 100.0).round() / 100.0,
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
            recording_frames: None,
            width: geometry.width,
            height: geometry.height,
            encoded_width: encoded.width,
            encoded_height: encoded.height,
        }
    }

    pub async fn ax_snapshot(&self, scan: bool) -> Result<AxSnapshot> {
        let (reply, response) = oneshot::channel();
        self.ax
            .send(AxCommand::Snapshot { scan, reply })
            .map_err(|_| anyhow!("Android accessibility worker stopped"))?;
        let snapshot = response
            .await
            .map_err(|_| anyhow!("Android accessibility worker stopped"))??;
        self.reconcile_orientation(snapshot.is_landscape);
        Ok(snapshot)
    }

    pub async fn ax_hit_test(&self, x: f64, y: f64) -> Result<Option<ElementDetail>> {
        let (reply, response) = oneshot::channel();
        self.ax
            .send(AxCommand::HitTest { x, y, reply })
            .map_err(|_| anyhow!("Android accessibility worker stopped"))?;
        response
            .await
            .map_err(|_| anyhow!("Android accessibility worker stopped"))?
    }

    pub async fn seed_orientation(&self) {
        if let Ok(snapshot) = self.ax_snapshot(false).await {
            self.reconcile_orientation(snapshot.is_landscape);
        }
    }

    fn reconcile_orientation(&self, is_landscape: bool) {
        let changed = {
            let mut orientation = self.orientation.lock().unwrap();
            if orientation.is_landscape() == is_landscape {
                false
            } else {
                *orientation = if is_landscape {
                    Orientation::LandscapeLeft
                } else {
                    Orientation::Portrait
                };
                true
            }
        };
        if changed {
            let _ = self.capture.set_landscape(is_landscape);
        }
    }
}
