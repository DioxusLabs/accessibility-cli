//! Layout of the Indigo HID messages SimulatorKit builds and the simulator
//! accepts.
//!
//! A message is a mach message: a header, then one or more fixed-stride
//! payloads. They are modelled here as `#[repr(C, packed(4))]` values so a
//! message can be edited by field and handed to the HID client by pointer. The
//! only unsafe code is [`take_builder_message`], which copies a builder's
//! buffer into one of these values and frees it, so no raw pointer outlives
//! that one function.
//!
//! Field names and constants come from idb's `FBSimulatorIndigoHID` and from
//! measuring what `IndigoHIDMessageForMouseNSEvent` returns; the `_unknown`
//! fields are carried through untouched.

use std::ffi::c_void;
use std::mem::{offset_of, size_of};

use super::common::ButtonDirection;
use super::hid::FINGERS;

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
}

/// `mach_msg_header_t`.
#[repr(C, packed(4))]
#[derive(Clone, Copy, Default)]
pub(super) struct MachMessageHeader {
    pub bits: u32,
    pub size: u32,
    pub remote_port: u32,
    pub local_port: u32,
    pub voucher_port: u32,
    pub id: i32,
}

/// The head of every Indigo message.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub(super) struct IndigoHeader {
    pub mach: MachMessageHeader,
    /// Size in bytes of each payload that follows.
    pub payload_stride: u32,
    /// idb calls this the event type (1 button, 2 touch). The two-finger
    /// builder sets it to 3, which is also that message's payload count.
    pub kind: u8,
    _reserved: [u8; 3],
}

impl IndigoHeader {
    /// Number of payloads the header describes.
    fn payload_count(&self) -> usize {
        usize::from(self.kind)
    }
}

/// One touch of a payload. Coordinates are 0..1 ratios of the raw framebuffer.
#[repr(C, packed(4))]
#[derive(Clone, Copy, Default)]
pub(super) struct IndigoTouch {
    /// idb sets this and `state2` to 1 on down and 0 on up.
    pub state: u32,
    pub state2: u32,
    /// Which screen edge the gesture started from, as the builder encodes it.
    pub edge_flags: u32,
    pub x_ratio: f64,
    pub y_ratio: f64,
    _unknown0: [f64; 3],
    pub touching: u32,
    pub in_range: u32,
    _unknown1: [u32; 3],
    _unknown2: [f64; 5],
}

/// One payload of a message.
#[repr(C, packed(4))]
#[derive(Clone, Copy, Default)]
pub(super) struct IndigoPayload {
    pub kind: u32,
    /// `mach_absolute_time` of the event.
    pub timestamp: u64,
    _unknown: u32,
    pub touch: IndigoTouch,
}

/// A payload of the two-finger message, which uses a longer stride.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub(super) struct MultiTouchPayload {
    pub payload: IndigoPayload,
    _tail: [u8; 0x20],
}

/// The message idb sends for a one-finger touch: a payload and a copy of it.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub(super) struct SingleTouchMessage {
    header: IndigoHeader,
    payloads: [IndigoPayload; 2],
    _tail: [u8; 0x20],
}

/// The message `IndigoHIDMessageForMouseNSEvent` returns for one point: a
/// hand payload and one finger payload. It is only used as a template.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub(super) struct SinglePointTemplate {
    header: IndigoHeader,
    hand: MultiTouchPayload,
    pub finger: MultiTouchPayload,
}

/// The message `IndigoHIDMessageForMouseNSEvent` returns for two points: a
/// hand payload followed by one payload per finger.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
pub(super) struct MultiTouchMessage {
    header: IndigoHeader,
    hand: MultiTouchPayload,
    fingers: [MultiTouchPayload; FINGERS],
}

/// A message layout `IndigoHIDMessageForMouseNSEvent` can return.
pub(super) trait BuilderLayout: Copy {
    const PAYLOAD_STRIDE: u32;
    const PAYLOAD_COUNT: usize;
}

impl BuilderLayout for SinglePointTemplate {
    const PAYLOAD_STRIDE: u32 = MULTI_TOUCH_PAYLOAD_STRIDE;
    const PAYLOAD_COUNT: usize = 2;
}

impl BuilderLayout for MultiTouchMessage {
    const PAYLOAD_STRIDE: u32 = MULTI_TOUCH_PAYLOAD_STRIDE;
    const PAYLOAD_COUNT: usize = FINGERS + 1;
}

pub(super) const SINGLE_TOUCH_PAYLOAD_STRIDE: u32 = size_of::<IndigoPayload>() as u32;
pub(super) const MULTI_TOUCH_PAYLOAD_STRIDE: u32 = size_of::<MultiTouchPayload>() as u32;

const INDIGO_EVENT_TYPE_TOUCH: u8 = 2;
const TOUCH_PAYLOAD_KIND: u32 = 0x0b;

const _: () = {
    assert!(size_of::<MachMessageHeader>() == 0x18);
    assert!(size_of::<IndigoHeader>() == 0x20);
    assert!(offset_of!(IndigoHeader, payload_stride) == 0x18);
    assert!(offset_of!(IndigoHeader, kind) == 0x1c);
    assert!(size_of::<IndigoTouch>() == 0x70);
    assert!(offset_of!(IndigoTouch, edge_flags) == 0x08);
    assert!(offset_of!(IndigoTouch, x_ratio) == 0x0c);
    assert!(offset_of!(IndigoTouch, y_ratio) == 0x14);
    assert!(offset_of!(IndigoTouch, touching) == 0x34);
    assert!(offset_of!(IndigoTouch, in_range) == 0x38);
    assert!(offset_of!(IndigoPayload, timestamp) == 0x04);
    assert!(offset_of!(IndigoPayload, touch) == 0x10);
    assert!(SINGLE_TOUCH_PAYLOAD_STRIDE == 0x80);
    assert!(MULTI_TOUCH_PAYLOAD_STRIDE == 0xa0);
    assert!(size_of::<SingleTouchMessage>() == 0x140);
    assert!(offset_of!(SingleTouchMessage, payloads) == 0x20);
    assert!(size_of::<SinglePointTemplate>() == 0x20 + 2 * 0xa0);
    assert!(offset_of!(SinglePointTemplate, finger) == 0x20 + 0xa0);
    assert!(size_of::<MultiTouchMessage>() == 0x20 + (FINGERS + 1) * 0xa0);
    assert!(offset_of!(MultiTouchMessage, hand) == 0x20);
};

/// Copy a touch builder's message into a `T` and free the builder's buffer.
///
/// `None` if the builder returned null or a message whose header does not
/// describe `T`'s layout. The header's payload stride and count are the only
/// size information the builder provides: it leaves the mach `size` zero.
pub(super) fn take_builder_message<T: BuilderLayout>(message: *mut c_void) -> Option<T> {
    if message.is_null() {
        return None;
    }
    // SAFETY: a non-null builder result is a `malloc`ed message that starts
    // with a header, and is `T` long when the header says so.
    unsafe {
        let header: IndigoHeader = std::ptr::read_unaligned(message.cast());
        let matches = header.payload_stride == T::PAYLOAD_STRIDE
            && header.payload_count() == T::PAYLOAD_COUNT;
        let value = matches.then(|| std::ptr::read_unaligned(message.cast::<T>()));
        libc::free(message);
        value
    }
}

impl SingleTouchMessage {
    /// Build idb's one-finger message around the hand touch of a builder
    /// template.
    pub(super) fn from_template(
        template: &SinglePointTemplate,
        x_ratio: f64,
        y_ratio: f64,
        direction: ButtonDirection,
    ) -> Self {
        let mut header = template.header;
        header.payload_stride = SINGLE_TOUCH_PAYLOAD_STRIDE;
        header.kind = INDIGO_EVENT_TYPE_TOUCH;

        let mut touch = template.hand.payload.touch;
        touch.x_ratio = x_ratio;
        touch.y_ratio = y_ratio;
        let state = match direction {
            ButtonDirection::Down => 1,
            ButtonDirection::Up => 0,
        };
        touch.state = state;
        touch.state2 = state;

        let first = IndigoPayload {
            kind: TOUCH_PAYLOAD_KIND,
            // SAFETY: no preconditions.
            timestamp: unsafe { mach_absolute_time() },
            _unknown: 0,
            touch,
        };
        let mut second = first;
        second.touch.state = 1;
        second.touch.state2 = 2;

        Self {
            header,
            payloads: [first, second],
            _tail: [0; 0x20],
        }
    }
}

/// The state of one finger in a [`MultiTouchMessage`].
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct FingerState {
    pub x_ratio: f64,
    pub y_ratio: f64,
    pub touching: bool,
    pub edge_flags: u32,
}

impl MultiTouchMessage {
    /// Rewrite the fingers in place. Fingers lift individually through their
    /// touching/in-range fields, and the hand stays touching while any is.
    pub(super) fn set_fingers(&mut self, fingers: [FingerState; FINGERS]) {
        let any_touching = u32::from(fingers.iter().any(|finger| finger.touching));
        self.hand.payload.touch.touching = any_touching;
        self.hand.payload.touch.in_range = any_touching;

        for (slot, finger) in self.fingers.iter_mut().zip(fingers) {
            let touch = &mut slot.payload.touch;
            touch.edge_flags = finger.edge_flags;
            touch.x_ratio = finger.x_ratio;
            touch.y_ratio = finger.y_ratio;
            touch.touching = u32::from(finger.touching);
            touch.in_range = u32::from(finger.touching);
        }
    }
}
