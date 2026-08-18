//! Compatibility re-exports for reusable iOS Simulator sessions.

use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use accessibility_core::platform::android::input as android_input;
use accessibility_core::platform::android::session as android_session;
use accessibility_core::video::{EncodedFrame, Recording, RecordingConfig};

#[cfg(target_os = "macos")]
pub use accessibility_core::platform::ios_simulator::session::SimSession;
#[cfg(target_os = "macos")]
use accessibility_core::platform::ios_simulator::{input as ios_input, session as ios_session};

pub use accessibility_core::platform::android::session::EmulatorSession;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    Portrait,
    PortraitUpsideDown,
    LandscapeLeft,
    LandscapeRight,
}

impl Orientation {
    pub fn is_landscape(self) -> bool {
        matches!(self, Self::LandscapeLeft | Self::LandscapeRight)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfo {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub orientation: Orientation,
    pub platform: &'static str,
}

#[derive(Debug, Clone, Serialize)]
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

#[derive(Clone)]
pub enum Session {
    #[cfg(target_os = "macos")]
    Ios(Arc<SimSession>),
    Android(Arc<EmulatorSession>),
}

impl Session {
    pub fn android(session: Arc<EmulatorSession>) -> Self {
        Self::Android(session)
    }

    #[cfg(target_os = "macos")]
    pub fn ios(session: Arc<SimSession>) -> Self {
        Self::Ios(session)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EncodedFrame> {
        match self {
            #[cfg(target_os = "macos")]
            Self::Ios(session) => session.subscribe(),
            Self::Android(session) => session.subscribe(),
        }
    }

    pub fn request_keyframe(&self) {
        match self {
            #[cfg(target_os = "macos")]
            Self::Ios(session) => session.request_keyframe(),
            Self::Android(session) => session.request_keyframe(),
        }
    }

    pub fn note_lag(&self) {
        match self {
            #[cfg(target_os = "macos")]
            Self::Ios(session) => session.note_lag(),
            Self::Android(session) => session.note_lag(),
        }
    }

    pub fn device_info(&self) -> DeviceInfo {
        match self {
            #[cfg(target_os = "macos")]
            Self::Ios(session) => {
                let device = session.device_info();
                DeviceInfo {
                    id: device.udid,
                    width: device.width,
                    height: device.height,
                    orientation: from_ios_orientation(device.orientation),
                    platform: "ios_simulator",
                }
            }
            Self::Android(session) => {
                let device = session.device_info();
                DeviceInfo {
                    id: device.serial,
                    width: device.width,
                    height: device.height,
                    orientation: from_android_orientation(device.orientation),
                    platform: "android_emulator",
                }
            }
        }
    }

    pub fn stats(&self) -> StatsReport {
        match self {
            #[cfg(target_os = "macos")]
            Self::Ios(session) => from_ios_stats(session.stats()),
            Self::Android(session) => from_android_stats(session.stats()),
        }
    }

    pub fn send_input_json(&self, payload: &str) -> Result<()> {
        match self {
            #[cfg(target_os = "macos")]
            Self::Ios(session) => {
                session.send_input(serde_json::from_str::<ios_input::InputCommand>(payload)?);
            }
            Self::Android(session) => {
                session.send_input(serde_json::from_str::<android_input::InputCommand>(
                    payload,
                )?);
            }
        }
        Ok(())
    }

    pub fn set_orientation(&self, orientation: Orientation) -> Result<()> {
        match self {
            #[cfg(target_os = "macos")]
            Self::Ios(session) => {
                session.set_orientation(to_ios_orientation(orientation));
                Ok(())
            }
            Self::Android(session) => session.set_orientation(to_android_orientation(orientation)),
        }
    }

    pub async fn seed_orientation(&self) {
        match self {
            #[cfg(target_os = "macos")]
            Self::Ios(session) => session.seed_orientation().await,
            Self::Android(session) => session.seed_orientation().await,
        }
    }

    pub async fn ax_snapshot(&self, scan: bool) -> Result<serde_json::Value> {
        match self {
            #[cfg(target_os = "macos")]
            Self::Ios(session) => Ok(serde_json::to_value(session.ax_snapshot(scan).await?)?),
            Self::Android(session) => Ok(serde_json::to_value(session.ax_snapshot(scan).await?)?),
        }
    }

    pub async fn ax_hit_test(&self, x: f64, y: f64) -> Result<serde_json::Value> {
        match self {
            #[cfg(target_os = "macos")]
            Self::Ios(session) => Ok(serde_json::to_value(session.ax_hit_test(x, y).await?)?),
            Self::Android(session) => Ok(serde_json::to_value(session.ax_hit_test(x, y).await?)?),
        }
    }

    pub fn start_recording(&self, config: RecordingConfig) -> Result<std::path::PathBuf> {
        match self {
            #[cfg(target_os = "macos")]
            Self::Ios(session) => session.start_recording(config),
            Self::Android(_) => {
                anyhow::bail!("recording is not supported for Android Emulator streams")
            }
        }
    }

    pub fn stop_recording(&self) -> Result<Recording> {
        match self {
            #[cfg(target_os = "macos")]
            Self::Ios(session) => session.stop_recording(),
            Self::Android(_) => {
                anyhow::bail!("recording is not supported for Android Emulator streams")
            }
        }
    }

    pub fn home_indicator_band(&self) -> f64 {
        match self {
            #[cfg(target_os = "macos")]
            Self::Ios(_) => ios_input::HOME_INDICATOR_BAND,
            Self::Android(_) => 1.0,
        }
    }

    #[cfg(target_os = "macos")]
    pub fn ios_session(&self) -> Option<&Arc<SimSession>> {
        match self {
            Self::Ios(session) => Some(session),
            Self::Android(_) => None,
        }
    }
}

fn from_android_orientation(orientation: android_input::Orientation) -> Orientation {
    match orientation {
        android_input::Orientation::Portrait => Orientation::Portrait,
        android_input::Orientation::PortraitUpsideDown => Orientation::PortraitUpsideDown,
        android_input::Orientation::LandscapeLeft => Orientation::LandscapeLeft,
        android_input::Orientation::LandscapeRight => Orientation::LandscapeRight,
    }
}

fn to_android_orientation(orientation: Orientation) -> android_input::Orientation {
    match orientation {
        Orientation::Portrait => android_input::Orientation::Portrait,
        Orientation::PortraitUpsideDown => android_input::Orientation::PortraitUpsideDown,
        Orientation::LandscapeLeft => android_input::Orientation::LandscapeLeft,
        Orientation::LandscapeRight => android_input::Orientation::LandscapeRight,
    }
}

fn from_android_stats(stats: android_session::StatsReport) -> StatsReport {
    StatsReport {
        uptime_secs: stats.uptime_secs,
        frames: stats.frames,
        keyframes: stats.keyframes,
        bytes: stats.bytes,
        fps: stats.fps,
        mbps: stats.mbps,
        bits_per_pixel: stats.bits_per_pixel,
        mean_frame_kb: stats.mean_frame_kb,
        keyframe_requests: stats.keyframe_requests,
        lag_events: stats.lag_events,
        subscribers: stats.subscribers,
        recording_frames: stats.recording_frames,
        width: stats.width,
        height: stats.height,
        encoded_width: stats.encoded_width,
        encoded_height: stats.encoded_height,
    }
}

#[cfg(target_os = "macos")]
fn from_ios_orientation(orientation: ios_input::Orientation) -> Orientation {
    match orientation {
        ios_input::Orientation::Portrait => Orientation::Portrait,
        ios_input::Orientation::PortraitUpsideDown => Orientation::PortraitUpsideDown,
        ios_input::Orientation::LandscapeLeft => Orientation::LandscapeLeft,
        ios_input::Orientation::LandscapeRight => Orientation::LandscapeRight,
    }
}

#[cfg(target_os = "macos")]
fn to_ios_orientation(orientation: Orientation) -> ios_input::Orientation {
    match orientation {
        Orientation::Portrait => ios_input::Orientation::Portrait,
        Orientation::PortraitUpsideDown => ios_input::Orientation::PortraitUpsideDown,
        Orientation::LandscapeLeft => ios_input::Orientation::LandscapeLeft,
        Orientation::LandscapeRight => ios_input::Orientation::LandscapeRight,
    }
}

#[cfg(target_os = "macos")]
fn from_ios_stats(stats: ios_session::StatsReport) -> StatsReport {
    StatsReport {
        uptime_secs: stats.uptime_secs,
        frames: stats.frames,
        keyframes: stats.keyframes,
        bytes: stats.bytes,
        fps: stats.fps,
        mbps: stats.mbps,
        bits_per_pixel: stats.bits_per_pixel,
        mean_frame_kb: stats.mean_frame_kb,
        keyframe_requests: stats.keyframe_requests,
        lag_events: stats.lag_events,
        subscribers: stats.subscribers,
        recording_frames: stats.recording_frames,
        width: stats.width,
        height: stats.height,
        encoded_width: stats.encoded_width,
        encoded_height: stats.encoded_height,
    }
}
