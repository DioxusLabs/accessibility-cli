use std::io::Read;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use accessibility_android_sys::AdbClient;
use accessibility_android_sys::emulator::screenrecord::{
    AnnexBAccessUnitParser, ScreenRecordConfig, spawn_screenrecord,
};
use anyhow::{Result, anyhow, bail};

use crate::video::{
    EncodedFrame, FrameKind, FrameSink, NalFormat, ScreenGeometry, Tuning, VideoCapture,
    VideoConfig,
};

const IDLE_FLUSH: Duration = Duration::from_millis(75);
const RESTART_DELAY: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy)]
enum CaptureControl {
    Restart(ScreenRecordConfig),
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureOutcome {
    Restart(Option<ScreenRecordConfig>),
    Stop,
}

pub struct AndroidVideoCapture {
    geometry: ScreenGeometry,
    encoded_geometry: std::sync::Mutex<ScreenGeometry>,
    capture_config: std::sync::Mutex<ScreenRecordConfig>,
    max_dimension: Option<u32>,
    bit_rate: u32,
    control: SyncSender<CaptureControl>,
    worker: Option<JoinHandle<()>>,
}

impl AndroidVideoCapture {
    pub fn start(
        adb: AdbClient,
        geometry: ScreenGeometry,
        config: &VideoConfig,
        sink: FrameSink,
    ) -> Result<Self> {
        if !geometry.is_valid() {
            bail!("Android Emulator screen geometry is unavailable");
        }
        if config.nal_format != NalFormat::AnnexB {
            bail!("Android Emulator screenrecord capture requires Annex-B H.264");
        }
        let bit_rate = match config.tuning {
            Tuning::Interactive { bitrate } => bitrate
                .unwrap_or_else(|| derived_bit_rate(geometry, config.max_dimension, config.fps)),
            Tuning::Recording { .. } => {
                bail!("Android Emulator live capture does not support recording tuning")
            }
        };
        let capture_config = ScreenRecordConfig::for_max_dimension(
            geometry.width,
            geometry.height,
            config.max_dimension,
            bit_rate,
        );
        let encoded_geometry = ScreenGeometry {
            width: capture_config.width,
            height: capture_config.height,
        };
        let first_child = spawn_screenrecord(&adb, capture_config)?;
        let (control, commands) = mpsc::sync_channel(4);
        let worker = std::thread::Builder::new()
            .name("android-screenrecord".into())
            .spawn(move || run_worker(adb, capture_config, first_child, commands, sink))?;
        Ok(Self {
            geometry,
            encoded_geometry: std::sync::Mutex::new(encoded_geometry),
            capture_config: std::sync::Mutex::new(capture_config),
            max_dimension: config.max_dimension,
            bit_rate,
            control,
            worker: Some(worker),
        })
    }

    pub fn set_landscape(&self, landscape: bool) -> Result<()> {
        let (width, height) = if landscape {
            (self.geometry.height, self.geometry.width)
        } else {
            (self.geometry.width, self.geometry.height)
        };
        let config =
            ScreenRecordConfig::for_max_dimension(width, height, self.max_dimension, self.bit_rate);
        self.control
            .send(CaptureControl::Restart(config))
            .map_err(|_| anyhow!("Android screenrecord worker stopped"))?;
        *self.capture_config.lock().unwrap() = config;
        *self.encoded_geometry.lock().unwrap() = ScreenGeometry {
            width: config.width,
            height: config.height,
        };
        Ok(())
    }

    fn stop_worker(&mut self) {
        let _ = self.control.send(CaptureControl::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl VideoCapture for AndroidVideoCapture {
    fn geometry(&self) -> ScreenGeometry {
        self.geometry
    }

    fn encoded_geometry(&self) -> ScreenGeometry {
        *self.encoded_geometry.lock().unwrap()
    }

    fn request_keyframe(&self) {
        let config = *self.capture_config.lock().unwrap();
        let _ = self.control.try_send(CaptureControl::Restart(config));
    }

    fn stop(&mut self) {
        self.stop_worker();
    }
}

impl Drop for AndroidVideoCapture {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

fn run_worker(
    adb: AdbClient,
    mut config: ScreenRecordConfig,
    first_child: std::process::Child,
    commands: Receiver<CaptureControl>,
    sink: FrameSink,
) {
    let mut next_child = Some(first_child);
    loop {
        let child = match next_child.take() {
            Some(child) => child,
            None => match spawn_screenrecord(&adb, config) {
                Ok(child) => child,
                Err(_) => {
                    match commands.recv_timeout(RESTART_DELAY) {
                        Ok(CaptureControl::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            return;
                        }
                        Ok(CaptureControl::Restart(next)) => config = next,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    continue;
                }
            },
        };
        match pump_child(child, &commands, &sink) {
            CaptureOutcome::Stop => return,
            CaptureOutcome::Restart(Some(next)) => config = next,
            CaptureOutcome::Restart(None) => {}
        }
        match commands.recv_timeout(RESTART_DELAY) {
            Ok(CaptureControl::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Ok(CaptureControl::Restart(next)) => config = next,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn pump_child(
    mut child: std::process::Child,
    commands: &Receiver<CaptureControl>,
    sink: &FrameSink,
) -> CaptureOutcome {
    let Some(mut stdout) = child.stdout.take() else {
        return CaptureOutcome::Restart(None);
    };
    let stderr = child.stderr.take();
    let (chunks_tx, chunks_rx) = mpsc::sync_channel::<Vec<u8>>(8);
    let reader = std::thread::spawn(move || {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            match stdout.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if chunks_tx.send(buffer[..read].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let stderr_reader = stderr.map(|mut stderr| {
        std::thread::spawn(move || {
            let mut output = Vec::new();
            let _ = stderr.read_to_end(&mut output);
            output
        })
    });

    let mut parser = AnnexBAccessUnitParser::default();
    let outcome = loop {
        match commands.try_recv() {
            Ok(CaptureControl::Stop) | Err(mpsc::TryRecvError::Disconnected) => {
                break CaptureOutcome::Stop;
            }
            Ok(CaptureControl::Restart(next)) => break CaptureOutcome::Restart(Some(next)),
            Err(mpsc::TryRecvError::Empty) => {}
        }
        match chunks_rx.recv_timeout(IDLE_FLUSH) {
            Ok(chunk) => emit(parser.push(&chunk), sink),
            Err(mpsc::RecvTimeoutError::Timeout) => emit(parser.flush_idle(), sink),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                emit(parser.flush_idle(), sink);
                break CaptureOutcome::Restart(None);
            }
        }
    };

    drop(chunks_rx);
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();
    if let Some(stderr_reader) = stderr_reader {
        let _ = stderr_reader.join();
    }
    outcome
}

fn emit(
    frames: Vec<accessibility_android_sys::emulator::screenrecord::H264AccessUnit>,
    sink: &FrameSink,
) {
    for frame in frames {
        sink(EncodedFrame {
            data: frame.data,
            kind: if frame.keyframe {
                FrameKind::Keyframe
            } else {
                FrameKind::Delta
            },
            captured_at: Instant::now(),
        });
    }
}

fn derived_bit_rate(geometry: ScreenGeometry, max_dimension: Option<u32>, fps: u32) -> u32 {
    let size =
        ScreenRecordConfig::for_max_dimension(geometry.width, geometry.height, max_dimension, 1);
    let bits = size.width as f64 * size.height as f64 * fps.max(1) as f64 * 0.15;
    bits.round().clamp(1_000_000.0, 24_000_000.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_bitrate_from_encoded_geometry() {
        let bitrate = derived_bit_rate(
            ScreenGeometry {
                width: 1080,
                height: 2424,
            },
            Some(1280),
            60,
        );
        assert_eq!(bitrate, 6_566_400);
    }
}
