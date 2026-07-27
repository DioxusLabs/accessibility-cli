//! Input forwarding from the browser to the simulator's HID subsystem.
//!
//! All coordinates on this path are normalized 0..1 fractions of the *raw*
//! framebuffer. The browser un-rotates them before sending, so nothing here
//! needs to know about orientation, and no points/pixels/scale conversion is
//! involved anywhere.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::Deserialize;

/// Touches below this fraction of the screen height are tagged as originating
/// from the bottom edge, which is what makes swipe-up-to-home work. Without
/// the edge hint iOS treats the drag as a normal in-app gesture.
pub const HOME_INDICATOR_BAND: f64 = 0.93;

/// How far a wheel delta moves the virtual finger, as a multiple of the
/// delta itself.
const SCROLL_GAIN: f64 = 1.0;

/// How close to an edge the virtual finger may get before the drag is
/// restarted from the middle of the screen.
const SCROLL_EDGE_MARGIN: f64 = 0.08;

/// Quiet period after which a scroll gesture is lifted.
const SCROLL_IDLE: Duration = Duration::from_millis(100);

/// How often the worker wakes to check for an expired scroll gesture.
const WORKER_TICK: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TouchPhase {
    Begin,
    Move,
    End,
}

/// Raw-framebuffer edge a touch is flagged as coming from.
///
/// Required for iOS to recognize system gestures such as swipe-up-to-home.
/// The client decides this, because only it knows the current orientation and
/// the framebuffer never rotates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TouchEdge {
    #[default]
    None,
    Left,
    Top,
    Bottom,
    Right,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    Portrait,
    PortraitUpsideDown,
    LandscapeLeft,
    LandscapeRight,
}

impl Orientation {
    /// Whether the display is wider than it is tall in this orientation.
    pub fn is_landscape(self) -> bool {
        matches!(
            self,
            Orientation::LandscapeLeft | Orientation::LandscapeRight
        )
    }
}

/// A single input action to apply to the simulator.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputCommand {
    Touch {
        phase: TouchPhase,
        x: f64,
        y: f64,
        #[serde(default)]
        edge: TouchEdge,
    },
    Button {
        button: HardwareButton,
    },
    /// A single key press by USB HID usage code, with optional held modifiers.
    ///
    /// Used for navigation and shortcuts; text goes through [`InputCommand::Text`]
    /// so the character-to-key table lives in one place.
    Key {
        key_code: u32,
        #[serde(default)]
        modifiers: Vec<u32>,
    },
    /// Type a string, expanded server-side into key presses.
    Text {
        text: String,
    },
    /// A wheel or trackpad delta, as a fraction of the display.
    Scroll {
        dx: f64,
        dy: f64,
        x: f64,
        y: f64,
    },
    Rotate {
        orientation: Orientation,
    },
}

/// Turns a stream of wheel deltas into a touch drag.
///
/// iOS has no notion of a scroll wheel, so scrolling has to be a finger. The
/// awkward part is that a real finger runs out of screen: once the virtual
/// contact point nears an edge it is lifted and re-planted in the middle, so
/// an unbounded wheel can keep producing motion.
#[derive(Default)]
struct ScrollGesture {
    active: bool,
    x: f64,
    y: f64,
    last_event: Option<Instant>,
}

impl ScrollGesture {
    fn near_edge(&self) -> bool {
        self.x < SCROLL_EDGE_MARGIN
            || self.x > 1.0 - SCROLL_EDGE_MARGIN
            || self.y < SCROLL_EDGE_MARGIN
            || self.y > 1.0 - SCROLL_EDGE_MARGIN
    }
}

/// Start the HID worker thread and return its command channel.
///
/// The worker owns the `SimulatorHID` because it is not `Sync`, and because
/// HID sends block on a dispatch queue round trip. It wakes periodically even
/// when idle so a scroll gesture can be lifted after the wheel stops.
#[cfg(target_os = "macos")]
pub fn spawn_input_worker(udid: &str) -> Result<Sender<InputCommand>> {
    use accessibility_ios_sys::{
        HardwareButton as SysButton, Orientation as SysOrientation, SimulatorHID,
        TouchEdge as SysEdge, TouchPhase as SysPhase,
    };

    let hid = SimulatorHID::for_device(Some(udid))?;
    let (tx, rx) = mpsc::channel::<InputCommand>();

    std::thread::Builder::new()
        .name("sim-input".into())
        .spawn(move || {
            let mut scroll = ScrollGesture::default();

            loop {
                match rx.recv_timeout(WORKER_TICK) {
                    Ok(command) => {
                        // Any direct touch interrupts an in-flight scroll, or
                        // the two gestures would fight over the same finger.
                        if !matches!(command, InputCommand::Scroll { .. }) && scroll.active {
                            let _ = hid.touch_normalized(scroll.x, scroll.y, SysPhase::End);
                            scroll = ScrollGesture::default();
                        }

                        let result = match command {
                            InputCommand::Touch { phase, x, y, edge } => {
                                let phase = match phase {
                                    TouchPhase::Begin => SysPhase::Begin,
                                    TouchPhase::Move => SysPhase::Move,
                                    TouchPhase::End => SysPhase::End,
                                };
                                let edge = match edge {
                                    TouchEdge::None => SysEdge::None,
                                    TouchEdge::Left => SysEdge::Left,
                                    TouchEdge::Top => SysEdge::Top,
                                    TouchEdge::Bottom => SysEdge::Bottom,
                                    TouchEdge::Right => SysEdge::Right,
                                };
                                hid.touch_normalized_edge(x, y, phase, edge)
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
                            InputCommand::Key {
                                key_code,
                                ref modifiers,
                            } => hid.send_key_with_modifiers(key_code, modifiers),
                            InputCommand::Text { ref text } => type_text(&hid, text),
                            InputCommand::Rotate { orientation } => {
                                hid.set_orientation(match orientation {
                                    Orientation::Portrait => SysOrientation::Portrait,
                                    Orientation::PortraitUpsideDown => {
                                        SysOrientation::PortraitUpsideDown
                                    }
                                    Orientation::LandscapeLeft => SysOrientation::LandscapeLeft,
                                    Orientation::LandscapeRight => SysOrientation::LandscapeRight,
                                })
                            }
                            InputCommand::Scroll { dx, dy, x, y } => {
                                apply_scroll(&hid, &mut scroll, dx, dy, x, y)
                            }
                        };

                        if let Err(error) = result {
                            tracing::warn!("input event failed: {error}");
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        // Lift the finger once the wheel has gone quiet,
                        // otherwise the page keeps inertial-scrolling.
                        if scroll.active
                            && scroll
                                .last_event
                                .is_some_and(|at| at.elapsed() >= SCROLL_IDLE)
                        {
                            let _ = hid.touch_normalized(scroll.x, scroll.y, SysPhase::End);
                            scroll = ScrollGesture::default();
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }

            if scroll.active {
                let _ = hid.touch_normalized(scroll.x, scroll.y, SysPhase::End);
            }
        })?;

    Ok(tx)
}

/// Expand text into key presses and send them.
///
/// Rejects the whole string if any character is untypeable, so a partial or
/// subtly wrong string is never entered.
#[cfg(target_os = "macos")]
fn type_text(hid: &accessibility_ios_sys::SimulatorHID, text: &str) -> Result<()> {
    for stroke in crate::keymap::keystrokes_for(text)? {
        hid.send_key_with_modifiers(stroke.usage, &stroke.modifiers())?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn apply_scroll(
    hid: &accessibility_ios_sys::SimulatorHID,
    scroll: &mut ScrollGesture,
    dx: f64,
    dy: f64,
    x: f64,
    y: f64,
) -> Result<()> {
    use accessibility_ios_sys::TouchPhase as SysPhase;

    if !scroll.active {
        // Plant the finger under the pointer so the gesture lands on whatever
        // the user is actually hovering.
        scroll.x = x.clamp(0.0, 1.0);
        scroll.y = y.clamp(0.0, 1.0);
        scroll.active = true;
        hid.touch_normalized(scroll.x, scroll.y, SysPhase::Begin)?;
    }

    // Content follows the finger, so the finger moves opposite the wheel.
    scroll.x = (scroll.x - dx * SCROLL_GAIN).clamp(0.0, 1.0);
    scroll.y = (scroll.y - dy * SCROLL_GAIN).clamp(0.0, 1.0);
    scroll.last_event = Some(Instant::now());

    if scroll.near_edge() {
        // Out of room: lift and re-plant in the middle so the next delta has
        // somewhere to go.
        hid.touch_normalized(scroll.x, scroll.y, SysPhase::End)?;
        scroll.x = 0.5;
        scroll.y = 0.5;
        hid.touch_normalized(scroll.x, scroll.y, SysPhase::Begin)?;
        return Ok(());
    }

    hid.touch_normalized(scroll.x, scroll.y, SysPhase::Move)
}

#[cfg(not(target_os = "macos"))]
pub fn spawn_input_worker(_udid: &str) -> Result<Sender<InputCommand>> {
    anyhow::bail!("Simulator input requires macOS")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gesture_detects_each_edge() {
        for (x, y) in [(0.01, 0.5), (0.99, 0.5), (0.5, 0.01), (0.5, 0.99)] {
            let gesture = ScrollGesture {
                active: true,
                x,
                y,
                last_event: None,
            };
            assert!(
                gesture.near_edge(),
                "expected ({x}, {y}) to be near an edge"
            );
        }
    }

    #[test]
    fn gesture_center_is_not_near_edge() {
        let gesture = ScrollGesture {
            active: true,
            x: 0.5,
            y: 0.5,
            last_event: None,
        };
        assert!(!gesture.near_edge());
    }

    #[test]
    fn landscape_classification() {
        assert!(Orientation::LandscapeLeft.is_landscape());
        assert!(Orientation::LandscapeRight.is_landscape());
        assert!(!Orientation::Portrait.is_landscape());
        assert!(!Orientation::PortraitUpsideDown.is_landscape());
    }
}
