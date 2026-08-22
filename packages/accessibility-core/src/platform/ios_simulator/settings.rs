//! Simulator-wide UI settings.
//!
//! These go through CoreSimulator's direct `SimDevice` interface, which is the
//! same mechanism Xcode's Devices window uses. Only the three options exposed
//! by that interface are supported: appearance, increase contrast, and content
//! size.
//!
//! The Devices window also offers reduce-motion, colour filters, transparency
//! and VoiceOver, but `SimDevice` has no selector for those — they require a
//! helper binary spawned *inside* the simulator that drives the private
//! libAccessibility setters. That is a meaningfully larger piece of work and
//! is deliberately not attempted here.

use std::time::Duration;

use accessibility_ios_sys::{
    SimulatorAppearance, SimulatorContentSize, SimulatorDevice, SimulatorIncreaseContrast,
};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

const CONTROL_TIMEOUT: Duration = Duration::from_secs(2);
const CONTROL_QUEUE_CAPACITY: usize = 16;

/// Content size categories, smallest to largest.
///
/// The five `accessibility-*` entries are the extended range that only appears
/// once a user opts into larger accessibility text.
pub const CONTENT_SIZES: &[&str] = &[
    "extra-small",
    "small",
    "medium",
    "large",
    "extra-large",
    "extra-extra-large",
    "extra-extra-extra-large",
    "accessibility-medium",
    "accessibility-large",
    "accessibility-extra-large",
    "accessibility-extra-extra-large",
    "accessibility-extra-extra-extra-large",
];

pub const APPEARANCES: &[&str] = &["light", "dark"];
pub const TOGGLE: &[&str] = &["enabled", "disabled"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingKey {
    Appearance,
    IncreaseContrast,
    ContentSize,
}

impl SettingKey {
    pub fn allowed_values(self) -> &'static [&'static str] {
        match self {
            SettingKey::Appearance => APPEARANCES,
            SettingKey::IncreaseContrast => TOGGLE,
            SettingKey::ContentSize => CONTENT_SIZES,
        }
    }

    pub fn all() -> [SettingKey; 3] {
        [
            SettingKey::Appearance,
            SettingKey::IncreaseContrast,
            SettingKey::ContentSize,
        ]
    }

    fn read(self, device: &SimulatorDevice) -> Result<String> {
        let value = match self {
            Self::Appearance => match device.appearance()? {
                SimulatorAppearance::Light => "light",
                SimulatorAppearance::Dark => "dark",
            },
            Self::IncreaseContrast => match device.increase_contrast()? {
                SimulatorIncreaseContrast::Disabled => "disabled",
                SimulatorIncreaseContrast::Enabled => "enabled",
            },
            Self::ContentSize => CONTENT_SIZES[device.content_size()?.index()],
        };
        Ok(value.to_string())
    }

    fn write(self, device: &SimulatorDevice, value: &str) -> Result<String> {
        match self {
            Self::Appearance => {
                let appearance = match value {
                    "light" => SimulatorAppearance::Light,
                    "dark" => SimulatorAppearance::Dark,
                    _ => unreachable!("validated appearance"),
                };
                device.set_appearance(appearance)?;
            }
            Self::IncreaseContrast => {
                let contrast = match value {
                    "disabled" => SimulatorIncreaseContrast::Disabled,
                    "enabled" => SimulatorIncreaseContrast::Enabled,
                    _ => unreachable!("validated increase contrast"),
                };
                device.set_increase_contrast(contrast)?;
            }
            Self::ContentSize => {
                let size = match value {
                    "increment" => device.content_size()?.step(1),
                    "decrement" => device.content_size()?.step(-1),
                    value => {
                        let index = CONTENT_SIZES
                            .iter()
                            .position(|candidate| *candidate == value)
                            .expect("validated content size");
                        SimulatorContentSize::try_from(index as i64 + 1)?
                    }
                };
                device.set_content_size(size)?;
            }
        }
        self.read(device)
    }

    fn validate(self, value: &str) -> Result<()> {
        // `content_size` also accepts increment/decrement, which are not in the
        // reported value set but are the ergonomic way to drive it from a UI.
        let stepping =
            matches!(self, Self::ContentSize) && matches!(value, "increment" | "decrement");

        if !stepping && !self.allowed_values().contains(&value) {
            return Err(anyhow!(
                "'{value}' is not valid for {:?}; expected one of {}",
                self,
                self.allowed_values().join(", ")
            ));
        }
        Ok(())
    }
}

/// One setting and its current value, as reported by the simulator.
#[derive(Debug, Clone, Serialize)]
pub struct Setting {
    pub key: SettingKey,
    /// Current value, or `unsupported`/`unknown` if the runtime says so.
    pub value: String,
    pub allowed: &'static [&'static str],
}

impl Setting {
    fn read_all(device: &SimulatorDevice) -> Vec<Self> {
        SettingKey::all()
            .into_iter()
            .map(|key| Self {
                key,
                // A failed read is reported as unknown rather than failing the
                // whole request; one unsupported option should not blank the UI.
                value: key.read(device).unwrap_or_else(|_| "unknown".to_string()),
                allowed: key.allowed_values(),
            })
            .collect()
    }
}

enum ControlCommand {
    ReadAll {
        reply: oneshot::Sender<Vec<Setting>>,
    },
    Write {
        key: SettingKey,
        value: String,
        reply: oneshot::Sender<Result<String>>,
    },
}

impl ControlCommand {
    fn execute(self, device: &SimulatorDevice) {
        match self {
            Self::ReadAll { reply } => {
                let _ = reply.send(Setting::read_all(device));
            }
            Self::Write { key, value, reply } => {
                let _ = reply.send(key.write(device, &value));
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct SimulatorControl {
    commands: mpsc::Sender<ControlCommand>,
}

impl SimulatorControl {
    /// Attach the direct CoreSimulator control lane.
    ///
    /// Starts:
    ///
    /// 1. A dedicated worker thread that owns the `SimDevice` and serializes
    ///    its synchronous settings round trips.
    pub(super) fn start(udid: &str) -> Result<Self> {
        let device = SimulatorDevice::for_device(Some(udid))?;
        let (commands, mut receiver) = mpsc::channel::<ControlCommand>(CONTROL_QUEUE_CAPACITY);
        std::thread::Builder::new()
            .name("sim-control".into())
            .spawn(move || {
                while let Some(command) = receiver.blocking_recv() {
                    command.execute(&device);
                }
            })?;
        Ok(Self { commands })
    }

    /// Read every supported setting from the device.
    pub(super) async fn read_all(&self) -> Result<Vec<Setting>> {
        let (reply, response) = oneshot::channel();
        let request = async {
            self.commands
                .send(ControlCommand::ReadAll { reply })
                .await
                .map_err(|_| anyhow!("simulator control worker stopped"))?;
            response
                .await
                .map_err(|_| anyhow!("simulator control worker stopped"))
        };
        tokio::time::timeout(CONTROL_TIMEOUT, request)
            .await
            .map_err(|_| anyhow!("simulator settings read timed out"))?
    }

    pub(super) async fn write(&self, key: SettingKey, value: &str) -> Result<String> {
        key.validate(value)?;
        let (reply, response) = oneshot::channel();
        let request = async {
            self.commands
                .send(ControlCommand::Write {
                    key,
                    value: value.to_string(),
                    reply,
                })
                .await
                .map_err(|_| anyhow!("simulator control worker stopped"))?;
            response
                .await
                .map_err(|_| anyhow!("simulator control worker stopped"))?
        };
        tokio::time::timeout(CONTROL_TIMEOUT, request)
            .await
            .map_err(|_| anyhow!("simulator setting write timed out"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_values_outside_the_allowed_set() {
        let error = SettingKey::Appearance
            .validate("chartreuse")
            .expect_err("invalid appearance should be rejected");
        // Rejected locally, without sending anything to CoreSimulator.
        assert!(error.to_string().contains("chartreuse"));
    }

    #[test]
    fn content_size_accepts_stepping_verbs() {
        // These are not reported values, so they must be allowed explicitly.
        assert!(!CONTENT_SIZES.contains(&"increment"));
        for value in ["increment", "decrement"] {
            assert!(SettingKey::ContentSize.validate(value).is_ok());
        }
    }

    #[test]
    fn every_key_has_values() {
        for key in SettingKey::all() {
            assert!(!key.allowed_values().is_empty());
        }
    }
}
