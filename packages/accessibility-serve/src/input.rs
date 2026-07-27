//! Input forwarding from the browser to the simulator's HID subsystem.
//!
//! All coordinates on this path are normalized 0..1 fractions of the display.
//! Keeping them normalized end to end avoids the points-vs-pixels-vs-scale
//! conversions that the accessibility side has to deal with.

use anyhow::Result;
use serde::Deserialize;
use tokio::sync::mpsc;

/// Touches below this fraction of the screen height are tagged as originating
/// from the bottom edge, which is what makes swipe-up-to-home work. Without
/// the edge hint iOS treats the drag as a normal in-app gesture.
pub const HOME_INDICATOR_BAND: f64 = 0.93;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TouchPhase {
    Begin,
    Move,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareButton {
    Home,
    Lock,
    Siri,
    SideButton,
    ApplePay,
}

/// A single input action to apply to the simulator.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputCommand {
    Touch {
        phase: TouchPhase,
        x: f64,
        y: f64,
    },
    Button {
        button: HardwareButton,
    },
    /// A US-keyboard virtual key code (HIToolbox `Events.h`).
    Key {
        key_code: u32,
    },
}

/// Start the HID worker thread and return its command channel.
///
/// The worker owns the `SimulatorHID` because it is not `Sync`, and because
/// HID sends block on a dispatch queue round trip.
#[cfg(target_os = "macos")]
pub fn spawn_input_worker(udid: &str) -> Result<mpsc::UnboundedSender<InputCommand>> {
    use accessibility_ios_sys::{HardwareButton as SysButton, SimulatorHID, TouchPhase as SysPhase};

    let hid = SimulatorHID::for_device(Some(udid))?;
    let (tx, mut rx) = mpsc::unbounded_channel::<InputCommand>();

    std::thread::Builder::new()
        .name("sim-input".into())
        .spawn(move || {
            while let Some(command) = rx.blocking_recv() {
                let result = match command {
                    InputCommand::Touch { phase, x, y } => {
                        let phase = match phase {
                            TouchPhase::Begin => SysPhase::Begin,
                            TouchPhase::Move => SysPhase::Move,
                            TouchPhase::End => SysPhase::End,
                        };
                        hid.touch_normalized(x, y, phase)
                    }
                    InputCommand::Button { button } => {
                        let button = match button {
                            HardwareButton::Home => SysButton::Home,
                            HardwareButton::Lock => SysButton::Lock,
                            HardwareButton::Siri => SysButton::Siri,
                            HardwareButton::SideButton => SysButton::SideButton,
                            HardwareButton::ApplePay => SysButton::ApplePay,
                        };
                        hid.press_button(button, 0)
                    }
                    InputCommand::Key { key_code } => hid.send_key(key_code),
                };

                if let Err(error) = result {
                    tracing::warn!("input event failed: {error}");
                }
            }
        })?;

    Ok(tx)
}

#[cfg(not(target_os = "macos"))]
pub fn spawn_input_worker(_udid: &str) -> Result<mpsc::UnboundedSender<InputCommand>> {
    anyhow::bail!("Simulator input requires macOS")
}
