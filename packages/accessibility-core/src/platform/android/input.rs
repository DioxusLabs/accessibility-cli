use std::sync::mpsc;

use accessibility_android_sys::emulator::protocol::controller::input_event;
use accessibility_android_sys::emulator::protocol::controller::keyboard_event::{
    KeyCodeType, KeyEventType,
};
use accessibility_android_sys::emulator::protocol::controller::{
    InputEvent, KeyboardEvent, Touch, TouchEvent,
};
use accessibility_android_sys::emulator::{EmulatorGrpcClient, discover_emulator};
use accessibility_android_sys::{AdbClient, AndroidKeyCode};
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

use crate::video::ScreenGeometry;

const ACTIVE_PRESSURE: i32 = 0x7fff;
const TOUCH_SIZE: i32 = 8;

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
    Back,
    Lock,
    AppSwitch,
}

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

    fn android_rotation(self) -> u8 {
        match self {
            Self::Portrait => 0,
            Self::LandscapeLeft => 1,
            Self::PortraitUpsideDown => 2,
            Self::LandscapeRight => 3,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputCommand {
    Touch {
        phase: TouchPhase,
        x: f64,
        y: f64,
    },
    Key {
        key_code: u32,
        #[serde(default)]
        modifiers: Vec<u32>,
    },
    Text {
        text: String,
    },
    Scroll {
        dx: f64,
        dy: f64,
        x: f64,
        y: f64,
    },
    Button {
        button: HardwareButton,
    },
    Rotate {
        orientation: Orientation,
    },
}

pub async fn spawn_input_worker(
    serial: &str,
    geometry: ScreenGeometry,
) -> Result<UnboundedSender<InputCommand>> {
    let discovery = discover_emulator(Some(serial)).await?;
    let adb = AdbClient::discover(Some(serial));
    let (commands, mut command_rx) = tokio::sync::mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("android-emulator-input".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = ready_tx.send(Err(error.to_string()));
                    return;
                }
            };
            runtime.block_on(async move {
                let mut client = match EmulatorGrpcClient::connect(discovery).await {
                    Ok(client) => client,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                while let Some(command) = command_rx.recv().await {
                    let command = match command {
                        InputCommand::Rotate { orientation } => {
                            let _ = set_device_orientation(&adb, orientation).await;
                            continue;
                        }
                        InputCommand::Button { button } => {
                            apply_hardware_button(&adb, button).await;
                            continue;
                        }
                        command => command,
                    };
                    for event in to_events(command, geometry) {
                        if let Err(error) = client.send_input(event).await {
                            eprintln!("Android Emulator input failed: {error:#}");
                            return;
                        }
                    }
                }
            });
        })?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(commands),
        Ok(Err(error)) => Err(anyhow!(error)),
        Err(_) => Err(anyhow!(
            "Android Emulator input worker stopped during startup"
        )),
    }
}

fn to_events(command: InputCommand, geometry: ScreenGeometry) -> Vec<InputEvent> {
    match command {
        InputCommand::Touch { phase, x, y } => vec![touch_event(phase, x, y, geometry)],
        InputCommand::Key {
            key_code,
            modifiers,
        } => {
            let mut events = Vec::with_capacity(modifiers.len() * 2 + 2);
            events.extend(
                modifiers
                    .iter()
                    .map(|modifier| usb_key(*modifier, KeyEventType::Keydown)),
            );
            events.push(usb_key(key_code, KeyEventType::Keydown));
            events.push(usb_key(key_code, KeyEventType::Keyup));
            events.extend(
                modifiers
                    .iter()
                    .rev()
                    .map(|modifier| usb_key(*modifier, KeyEventType::Keyup)),
            );
            events
        }
        InputCommand::Text { text } => vec![keyboard_event(KeyboardEvent {
            text,
            ..Default::default()
        })],
        InputCommand::Scroll { dx, dy, x, y } => {
            let end_x = (x - dx).clamp(0.0, 1.0);
            let end_y = (y - dy).clamp(0.0, 1.0);
            vec![
                touch_event(TouchPhase::Begin, x, y, geometry),
                touch_event(TouchPhase::Move, end_x, end_y, geometry),
                touch_event(TouchPhase::End, end_x, end_y, geometry),
            ]
        }
        InputCommand::Button { .. } => Vec::new(),
        InputCommand::Rotate { .. } => Vec::new(),
    }
}

fn touch_event(phase: TouchPhase, x: f64, y: f64, geometry: ScreenGeometry) -> InputEvent {
    InputEvent {
        r#type: Some(input_event::Type::TouchEvent(TouchEvent {
            touches: vec![Touch {
                x: normalized_coordinate(x, geometry.width),
                y: normalized_coordinate(y, geometry.height),
                identifier: 0,
                pressure: if phase == TouchPhase::End {
                    0
                } else {
                    ACTIVE_PRESSURE
                },
                touch_major: TOUCH_SIZE,
                touch_minor: TOUCH_SIZE,
                expiration: 1,
                orientation: 0,
            }],
            display: 0,
        })),
    }
}

fn usb_key(key_code: u32, event_type: KeyEventType) -> InputEvent {
    let key_code = if key_code <= 0xffff {
        0x070000 | key_code
    } else {
        key_code
    };
    keyboard_event(KeyboardEvent {
        code_type: KeyCodeType::Usb as i32,
        event_type: event_type as i32,
        key_code: key_code as i32,
        ..Default::default()
    })
}

fn keyboard_event(event: KeyboardEvent) -> InputEvent {
    InputEvent {
        r#type: Some(input_event::Type::KeyEvent(event)),
    }
}

fn normalized_coordinate(value: f64, dimension: u32) -> i32 {
    (value.clamp(0.0, 1.0) * dimension.saturating_sub(1) as f64).round() as i32
}

async fn apply_hardware_button(adb: &AdbClient, button: HardwareButton) {
    let key = match button {
        HardwareButton::Home => AndroidKeyCode::Home,
        HardwareButton::Back => AndroidKeyCode::Back,
        HardwareButton::Lock => AndroidKeyCode::Power,
        HardwareButton::AppSwitch => AndroidKeyCode::AppSwitch,
    };
    let _ = adb.key_event(key as u32).await;
}

pub async fn set_device_orientation(adb: &AdbClient, orientation: Orientation) -> Result<()> {
    adb.shell(&["wm", "fixed-to-user-rotation", "enabled"])
        .await?;
    let target = orientation.android_rotation();
    adb.shell(&["wm", "user-rotation", "lock", &target.to_string()])
        .await?;
    for _ in 0..20 {
        let output = adb.shell(&["dumpsys", "display"]).await?;
        if display_rotation(&output) == Some(target) {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    bail!("Android display did not reach rotation {target}")
}

fn display_rotation(output: &str) -> Option<u8> {
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("mCurrentOrientation=")?
            .parse()
            .ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry() -> ScreenGeometry {
        ScreenGeometry {
            width: 1080,
            height: 2424,
        }
    }

    #[test]
    fn touch_maps_normalized_coordinates_and_pressure() {
        let events = to_events(
            InputCommand::Touch {
                phase: TouchPhase::Begin,
                x: 0.5,
                y: 1.0,
            },
            geometry(),
        );
        let Some(input_event::Type::TouchEvent(event)) = &events[0].r#type else {
            panic!("expected touch event");
        };
        assert_eq!(event.touches[0].x, 540);
        assert_eq!(event.touches[0].y, 2423);
        assert_eq!(event.touches[0].pressure, ACTIVE_PRESSURE);
    }

    #[test]
    fn parses_display_rotation() {
        assert_eq!(
            display_rotation("other\n    mCurrentOrientation=3\nmore"),
            Some(3)
        );
    }

    #[test]
    fn modifiers_are_held_around_key() {
        let events = to_events(
            InputCommand::Key {
                key_code: 4,
                modifiers: vec![225],
            },
            geometry(),
        );
        let kinds = events
            .iter()
            .map(|event| {
                let Some(input_event::Type::KeyEvent(key)) = &event.r#type else {
                    panic!("expected key event");
                };
                (key.key_code, key.event_type)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![(0x0700e1, 0), (0x070004, 0), (0x070004, 1), (0x0700e1, 1),]
        );
    }
}
