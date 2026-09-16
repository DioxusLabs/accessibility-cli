//! Input forwarding from the browser to the simulator's HID subsystem.
//!
//! All coordinates on this path are normalized 0..1 fractions of the *raw*
//! framebuffer. The browser un-rotates them before sending, so nothing here
//! needs to know about orientation, and no points/pixels/scale conversion is
//! involved anywhere.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use accessibility_ios_sys::{
    FINGERS, HardwareButton as SysButton, Orientation as SysOrientation, SimulatorHID,
    TouchContact as SysContact, TouchEdge as SysEdge,
};
use anyhow::Result;
use serde::Deserialize;

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
    VolumeUp,
    VolumeDown,
    Mute,
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

        /// The viewer's id for this contact, stable while it is held. Viewers
        /// from before multi-touch send none, and only ever hold one contact.
        #[serde(default)]
        id: u32,
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

/// What the simulator's HID interface supports on this host.
///
/// `IndigoHIDMessageForHIDArbitrary` is resolved at runtime, so buttons that
/// ride on it (volume, mute) are unavailable on Xcode versions without it.
#[derive(Debug, Clone, Copy, Default)]
pub struct InputCapabilities {
    pub arbitrary_hid: bool,
}

/// Start the HID worker thread and return its command channel plus the
/// capabilities the worker's `SimulatorHID` resolved.
///
/// The worker owns the `SimulatorHID` because it is not `Sync`, and because
/// HID sends block on a dispatch queue round trip. It sleeps on the channel,
/// waking early only to lift a scroll gesture once the wheel has stopped.
pub fn spawn_input_worker(udid: &str) -> Result<(Sender<InputCommand>, InputCapabilities)> {
    let hid = SimulatorHID::for_device(Some(udid))?;
    let capabilities = InputCapabilities {
        arbitrary_hid: hid.supports_hid_arbitrary(),
    };
    let (tx, rx) = mpsc::channel::<InputCommand>();

    let worker = InputWorker {
        hid,
        contacts: ContactSet::default(),
        scroll: None,
    };
    std::thread::Builder::new()
        .name("sim-input".into())
        .spawn(move || worker.run(rx))?;

    Ok((tx, capabilities))
}

/// The thread that applies [`InputCommand`]s to one simulator.
struct InputWorker {
    hid: SimulatorHID,
    contacts: ContactSet,

    /// The wheel gesture in progress, whose finger is held in `contacts`.
    scroll: Option<ScrollGesture>,
}

impl InputWorker {
    /// Apply commands until every sender is gone, then lift every finger.
    fn run(mut self, rx: Receiver<InputCommand>) {
        loop {
            // A wheel has no end event, so its finger lifts once the wheel has
            // been quiet this long; otherwise the page keeps inertial-scrolling.
            const SCROLL_IDLE: Duration = Duration::from_millis(100);

            let command = match self.scroll {
                Some(scroll) => {
                    let idle =
                        (scroll.last_event + SCROLL_IDLE).saturating_duration_since(Instant::now());
                    match rx.recv_timeout(idle) {
                        Ok(command) => command,
                        Err(RecvTimeoutError::Timeout) => {
                            if let Err(error) = self.lift_scroll() {
                                tracing::warn!("scroll lift failed: {error}");

                                // Retry after another idle period rather than spinning.
                                self.scroll = Some(ScrollGesture {
                                    last_event: Instant::now(),
                                    ..scroll
                                });
                            }
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                None => match rx.recv() {
                    Ok(command) => command,
                    Err(_) => break,
                },
            };

            if let Err(error) = self.apply(command) {
                tracing::warn!("input event failed: {error}");
            }
        }

        let _ = self.contacts.apply(ContactSet::lift_all, |fingers| {
            self.hid.touch_normalized(fingers.map(Slot::contact))
        });
    }

    fn apply(&mut self, command: InputCommand) -> Result<()> {
        // Any direct touch interrupts an in-flight scroll, or the two gestures
        // would fight over the screen.
        if !matches!(command, InputCommand::Scroll { .. }) {
            let lifted = self.lift_scroll();

            // A touch beside a wheel finger that would not lift is a pinch
            // nobody made, so it waits for the lift to be retried.
            if matches!(command, InputCommand::Touch { .. }) {
                lifted?;
            }
        }

        match command {
            InputCommand::Touch {
                phase,
                x,
                y,
                edge,
                id,
            } => self.touch(Holder::Viewer(id), phase, x, y, edge),
            InputCommand::Button { button } => {
                const CONSUMER_PAGE: u32 = 0x0c;
                const CONSUMER_MUTE: u32 = 0xe2;
                const CONSUMER_VOLUME_UP: u32 = 0xe9;
                const CONSUMER_VOLUME_DOWN: u32 = 0xea;

                match button {
                    HardwareButton::Home => self.hid.press_button(SysButton::Home, 0),
                    HardwareButton::Lock => self.hid.press_button(SysButton::Lock, 0),
                    HardwareButton::VolumeUp => {
                        self.hid
                            .press_hid_button(CONSUMER_PAGE, CONSUMER_VOLUME_UP, 0)
                    }
                    HardwareButton::VolumeDown => {
                        self.hid
                            .press_hid_button(CONSUMER_PAGE, CONSUMER_VOLUME_DOWN, 0)
                    }
                    HardwareButton::Mute => {
                        self.hid.press_hid_button(CONSUMER_PAGE, CONSUMER_MUTE, 0)
                    }
                    HardwareButton::Siri => self.hid.press_button(SysButton::Siri, 0),
                    HardwareButton::SideButton => self.hid.press_button(SysButton::SideButton, 0),
                    HardwareButton::ApplePay => self.hid.press_button(SysButton::ApplePay, 0),
                }
            }
            InputCommand::Key {
                key_code,
                ref modifiers,
            } => self.hid.send_key_with_modifiers(key_code, modifiers),
            InputCommand::Text { ref text } => type_text(&self.hid, text),
            InputCommand::Rotate { orientation } => self.hid.set_orientation(match orientation {
                Orientation::Portrait => SysOrientation::Portrait,
                Orientation::PortraitUpsideDown => SysOrientation::PortraitUpsideDown,
                Orientation::LandscapeLeft => SysOrientation::LandscapeLeft,
                Orientation::LandscapeRight => SysOrientation::LandscapeRight,
            }),
            InputCommand::Scroll { dx, dy, x, y } => self.scroll(dx, dy, x, y),
        }
    }

    /// Move one holder's finger, sending the event once the change is made.
    fn touch(
        &mut self,
        holder: Holder,
        phase: TouchPhase,
        x: f64,
        y: f64,
        edge: TouchEdge,
    ) -> Result<()> {
        self.contacts.apply(
            |contacts| contacts.update(holder, phase, x, y, edge),
            |fingers| self.hid.touch_normalized(fingers.map(Slot::contact)),
        )
    }

    /// Drive the wheel's finger by one wheel delta.
    fn scroll(&mut self, dx: f64, dy: f64, x: f64, y: f64) -> Result<()> {
        let mut scroll = match self.scroll {
            Some(scroll) => scroll,
            None => {
                // A wheel takes the screen from any held contacts, and plants
                // its finger under the pointer so the gesture lands on whatever
                // the user is actually hovering.
                self.contacts.apply(ContactSet::lift_all, |fingers| {
                    self.hid.touch_normalized(fingers.map(Slot::contact))
                })?;
                let (x, y) = (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
                self.touch(Holder::Scroll, TouchPhase::Begin, x, y, TouchEdge::None)?;
                ScrollGesture {
                    x,
                    y,
                    last_event: Instant::now(),
                }
            }
        };

        // Content follows the finger, so the finger moves opposite the wheel.
        scroll.x = (scroll.x - dx).clamp(0.0, 1.0);
        scroll.y = (scroll.y - dy).clamp(0.0, 1.0);
        scroll.last_event = Instant::now();
        self.scroll = Some(scroll);

        if !scroll.near_edge() {
            return self.touch(
                Holder::Scroll,
                TouchPhase::Move,
                scroll.x,
                scroll.y,
                TouchEdge::None,
            );
        }

        // Out of room: lift and re-plant in the middle so the next delta has
        // somewhere to go.
        self.touch(
            Holder::Scroll,
            TouchPhase::End,
            scroll.x,
            scroll.y,
            TouchEdge::None,
        )?;
        self.scroll = Some(ScrollGesture {
            x: 0.5,
            y: 0.5,
            ..scroll
        });
        self.touch(Holder::Scroll, TouchPhase::Begin, 0.5, 0.5, TouchEdge::None)
    }

    /// End the wheel gesture in progress, if any. It is only forgotten once
    /// its finger lifts, so a refused lift is retried.
    fn lift_scroll(&mut self) -> Result<()> {
        let Some(scroll) = self.scroll else {
            return Ok(());
        };

        self.touch(
            Holder::Scroll,
            TouchPhase::End,
            scroll.x,
            scroll.y,
            TouchEdge::None,
        )?;

        self.scroll = None;
        Ok(())
    }
}

/// Turns a stream of wheel deltas into a touch drag.
///
/// iOS has no notion of a scroll wheel, so scrolling has to be a finger. The
/// awkward part is that a real finger runs out of screen: once the virtual
/// contact point nears an edge it is lifted and re-planted in the middle, so
/// an unbounded wheel can keep producing motion.
#[derive(Debug, Clone, Copy)]
struct ScrollGesture {
    x: f64,
    y: f64,
    last_event: Instant,
}

impl ScrollGesture {
    fn near_edge(&self) -> bool {
        const MARGIN: f64 = 0.08;

        self.x < MARGIN || self.x > 1.0 - MARGIN || self.y < MARGIN || self.y > 1.0 - MARGIN
    }
}

/// The fingers held down on the simulator screen.
///
/// Indigo gives the fingers of a touch event fixed identities by index. A
/// holder takes the lowest free finger when it lands and keeps it until it
/// lifts, so a finger never jumps to a new identity mid-gesture. Every event
/// carries every finger, with the free ones lifted.
#[derive(Debug, Default, Clone, Copy)]
struct ContactSet {
    slots: [Slot; FINGERS],
}

impl ContactSet {
    fn is_idle(&self) -> bool {
        self.slots.iter().all(|slot| slot.holder.is_none())
    }

    /// Make a change once the simulator has taken the event expressing it.
    ///
    /// A finger whose lift the simulator refused stays held, so the next lift
    /// or the worker's shutdown tries again instead of forgetting it.
    fn apply(
        &mut self,
        change: impl FnOnce(&mut Self) -> Option<[Slot; FINGERS]>,
        inject: impl FnOnce([Slot; FINGERS]) -> Result<()>,
    ) -> Result<()> {
        let mut next = *self;
        if let Some(fingers) = change(&mut next) {
            inject(fingers)?;
        }
        *self = next;
        Ok(())
    }

    /// Apply one holder's touch and return the event that expresses the new
    /// state.
    fn update(
        &mut self,
        holder: Holder,
        phase: TouchPhase,
        x: f64,
        y: f64,
        edge: TouchEdge,
    ) -> Option<[Slot; FINGERS]> {
        let held = self
            .slots
            .iter()
            .position(|slot| slot.holder == Some(holder));
        let free = self.slots.iter().position(|slot| slot.holder.is_none());
        let index = match (held, phase) {
            (Some(index), _) => index,

            // An unknown holder lifting is sent through only while nothing is
            // held, which can only clear a stray touch.
            (None, TouchPhase::End) => {
                if !self.is_idle() {
                    return None;
                }
                0
            }
            (None, _) => {
                let Some(index) = free else {
                    tracing::debug!("ignoring {holder:?}: every finger is held");
                    return None;
                };
                index
            }
        };

        self.slots[index] = Slot {
            holder: (phase != TouchPhase::End).then_some(holder),
            x,
            y,
            edge,
        };
        Some(self.slots)
    }

    /// Lift every finger, returning the event that does so.
    fn lift_all(&mut self) -> Option<[Slot; FINGERS]> {
        if self.is_idle() {
            return None;
        }

        for slot in &mut self.slots {
            slot.holder = None;
        }
        Some(self.slots)
    }
}

/// One finger of an Indigo touch event.
///
/// A slot keeps its last position after its holder lifts, so it can be sent
/// as a lifted finger.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Slot {
    /// Who is holding this finger down, if anyone.
    holder: Option<Holder>,
    x: f64,
    y: f64,
    edge: TouchEdge,
}

impl Slot {
    fn contact(self) -> SysContact {
        SysContact {
            x: self.x,
            y: self.y,
            touching: self.holder.is_some(),
            edge: match self.edge {
                TouchEdge::None => SysEdge::None,
                TouchEdge::Left => SysEdge::Left,
                TouchEdge::Top => SysEdge::Top,
                TouchEdge::Bottom => SysEdge::Bottom,
                TouchEdge::Right => SysEdge::Right,
            },
        }
    }
}

/// What holds a finger down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holder {
    /// A viewer contact, by the id the viewer gave it.
    Viewer(u32),

    /// The virtual finger a wheel drives.
    Scroll,
}

/// Expand text into key presses and send them.
///
/// Rejects the whole string if any character is untypeable, so a partial or
/// subtly wrong string is never entered.
fn type_text(hid: &SimulatorHID, text: &str) -> Result<()> {
    for stroke in super::keymap::keystrokes_for(text)? {
        hid.send_key_with_modifiers(stroke.usage, &stroke.modifiers())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fingers of an event, each `(x, y, touching)`.
    fn fingers(fingers: [(f64, f64, bool); FINGERS]) -> Option<[SysContact; FINGERS]> {
        Some(fingers.map(|(x, y, touching)| SysContact {
            x,
            y,
            touching,
            edge: SysEdge::None,
        }))
    }

    fn touch(
        contacts: &mut ContactSet,
        id: u32,
        phase: TouchPhase,
        x: f64,
        y: f64,
    ) -> Option<[SysContact; FINGERS]> {
        contacts
            .update(Holder::Viewer(id), phase, x, y, TouchEdge::None)
            .map(|slots| slots.map(Slot::contact))
    }

    #[test]
    fn gesture_detects_each_edge() {
        for (x, y) in [(0.01, 0.5), (0.99, 0.5), (0.5, 0.01), (0.5, 0.99)] {
            let gesture = ScrollGesture {
                x,
                y,
                last_event: Instant::now(),
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
            x: 0.5,
            y: 0.5,
            last_event: Instant::now(),
        };
        assert!(!gesture.near_edge());
    }

    #[test]
    fn one_contact_sends_the_other_fingers_lifted() {
        let mut contacts = ContactSet::default();

        assert_eq!(
            touch(&mut contacts, 0, TouchPhase::Begin, 0.4, 0.5),
            fingers([(0.4, 0.5, true), (0.0, 0.0, false)])
        );
        assert_eq!(
            touch(&mut contacts, 0, TouchPhase::Move, 0.3, 0.5),
            fingers([(0.3, 0.5, true), (0.0, 0.0, false)])
        );
        assert_eq!(
            touch(&mut contacts, 0, TouchPhase::End, 0.3, 0.5),
            fingers([(0.3, 0.5, false), (0.0, 0.0, false)])
        );
        assert!(contacts.is_idle());
    }

    #[test]
    fn pinch_that_becomes_a_drag() {
        let mut contacts = ContactSet::default();

        touch(&mut contacts, 0, TouchPhase::Begin, 0.4, 0.5);
        assert_eq!(
            touch(&mut contacts, 1, TouchPhase::Begin, 0.6, 0.5),
            fingers([(0.4, 0.5, true), (0.6, 0.5, true)])
        );
        assert_eq!(
            touch(&mut contacts, 0, TouchPhase::End, 0.4, 0.5),
            fingers([(0.4, 0.5, false), (0.6, 0.5, true)])
        );
        assert_eq!(
            touch(&mut contacts, 1, TouchPhase::Move, 0.8, 0.5),
            fingers([(0.4, 0.5, false), (0.8, 0.5, true)])
        );
        assert_eq!(
            touch(&mut contacts, 1, TouchPhase::End, 0.8, 0.5),
            fingers([(0.4, 0.5, false), (0.8, 0.5, false)])
        );
        assert!(contacts.is_idle());
    }

    #[test]
    fn contacts_keep_their_finger() {
        let mut contacts = ContactSet::default();

        touch(&mut contacts, 7, TouchPhase::Begin, 0.4, 0.5);
        touch(&mut contacts, 3, TouchPhase::Begin, 0.6, 0.5);
        touch(&mut contacts, 7, TouchPhase::End, 0.4, 0.5);
        assert_eq!(
            touch(&mut contacts, 3, TouchPhase::Move, 0.7, 0.5),
            fingers([(0.4, 0.5, false), (0.7, 0.5, true)])
        );
        assert_eq!(
            touch(&mut contacts, 9, TouchPhase::Begin, 0.3, 0.5),
            fingers([(0.3, 0.5, true), (0.7, 0.5, true)])
        );
    }

    #[test]
    fn contacts_beyond_the_fingers_are_ignored() {
        let mut contacts = ContactSet::default();

        for id in 0..FINGERS as u32 {
            touch(&mut contacts, id, TouchPhase::Begin, 0.5, 0.5);
        }
        let before = contacts.slots;
        assert_eq!(touch(&mut contacts, 99, TouchPhase::Begin, 0.1, 0.1), None);
        assert_eq!(touch(&mut contacts, 99, TouchPhase::End, 0.1, 0.1), None);
        assert_eq!(contacts.slots, before);
    }

    #[test]
    fn stray_lift_is_sent_only_while_idle() {
        let mut contacts = ContactSet::default();

        assert_eq!(
            touch(&mut contacts, 4, TouchPhase::End, 0.5, 0.5),
            fingers([(0.5, 0.5, false), (0.0, 0.0, false)])
        );

        touch(&mut contacts, 0, TouchPhase::Begin, 0.4, 0.5);
        assert_eq!(touch(&mut contacts, 4, TouchPhase::End, 0.5, 0.5), None);
    }

    #[test]
    fn scroll_and_viewer_fingers_are_separate() {
        let mut contacts = ContactSet::default();

        contacts.update(Holder::Scroll, TouchPhase::Begin, 0.5, 0.5, TouchEdge::None);
        touch(&mut contacts, 0, TouchPhase::Begin, 0.4, 0.5);
        assert_eq!(contacts.slots[0].holder, Some(Holder::Scroll));
        assert_eq!(contacts.slots[1].holder, Some(Holder::Viewer(0)));
    }

    #[test]
    fn lift_all_releases_every_contact() {
        let mut contacts = ContactSet::default();
        assert_eq!(contacts.lift_all(), None);

        touch(&mut contacts, 0, TouchPhase::Begin, 0.4, 0.5);
        touch(&mut contacts, 1, TouchPhase::Begin, 0.6, 0.5);
        assert_eq!(
            contacts.lift_all().map(|slots| slots.map(Slot::contact)),
            fingers([(0.4, 0.5, false), (0.6, 0.5, false)])
        );
        assert!(contacts.is_idle());
    }

    #[test]
    fn refused_lift_keeps_the_finger_held() {
        let mut contacts = ContactSet::default();
        let accept = |_| Ok(());
        let refuse = |_| Err(anyhow::anyhow!("refused"));
        let viewer = |id| {
            move |c: &mut ContactSet| {
                c.update(
                    Holder::Viewer(id),
                    TouchPhase::Begin,
                    0.5,
                    0.5,
                    TouchEdge::None,
                )
            }
        };

        contacts.apply(viewer(0), accept).unwrap();
        contacts.apply(viewer(1), accept).unwrap();
        assert!(
            contacts
                .apply(
                    |c| c.update(
                        Holder::Viewer(1),
                        TouchPhase::End,
                        0.6,
                        0.5,
                        TouchEdge::None
                    ),
                    refuse
                )
                .is_err()
        );
        assert_eq!(contacts.slots[1].holder, Some(Holder::Viewer(1)));

        assert!(contacts.apply(ContactSet::lift_all, refuse).is_err());
        assert!(!contacts.is_idle());
        contacts.apply(ContactSet::lift_all, accept).unwrap();
        assert!(contacts.is_idle());
    }

    #[test]
    fn touch_id_defaults_to_the_first_contact() {
        let command: InputCommand =
            serde_json::from_str(r#"{"type":"touch","phase":"begin","x":0.5,"y":0.5}"#).unwrap();
        assert!(matches!(command, InputCommand::Touch { id: 0, .. }));

        let command: InputCommand =
            serde_json::from_str(r#"{"type":"touch","phase":"move","x":0.5,"y":0.5,"id":1}"#)
                .unwrap();
        assert!(matches!(command, InputCommand::Touch { id: 1, .. }));
    }

    #[test]
    fn landscape_classification() {
        assert!(Orientation::LandscapeLeft.is_landscape());
        assert!(Orientation::LandscapeRight.is_landscape());
        assert!(!Orientation::Portrait.is_landscape());
        assert!(!Orientation::PortraitUpsideDown.is_landscape());
    }
}
