//! iOS Simulator accessibility and HID support.
//!
//! This module provides:
//! - **Accessibility tree reading** for iOS apps via `AccessibilityPlatformTranslation` framework
//! - **HID injection** (taps, swipes, buttons) via the Indigo protocol and `SimulatorKit`
//!
//! # Accessibility Architecture
//!
//! ```text
//! Rust (IOSSimulatorAccessibility)
//!     ↓ objc2 FFI
//! AccessibilityPlatformTranslation.framework
//!     ↓
//! AXPTranslator singleton ← bridgeTokenDelegate (TranslationDispatcher)
//!     ↓
//! AXPMacPlatformElement
//!     ↓
//! CoreSimulator.framework → SimDevice.sendAccessibilityRequestAsync
//!     ↓
//! XPC → iOS Simulator
//! ```
//!
//! # HID Architecture (Indigo Protocol)
//!
//! ```text
//! Rust (SimulatorHID)
//!     ↓ objc2 FFI
//! SimulatorKit.framework → SimDeviceLegacyHIDClient
//!     ↓
//! IndigoMessage (binary protocol)
//!     ↓
//! Mach messaging → iOS Simulator HID subsystem
//! ```
//!
//! # Multi-Simulator Support
//!
//! The `AXPTranslator` is a singleton, so we use tokens to route requests to the correct
//! simulator. Each accessibility request gets a unique UUID token that maps to a `SimDevice`.

#![allow(unsafe_op_in_unsafe_fn)]

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::sync::{Arc, Mutex, OnceLock};

use crate::frameworks::load_simulatorkit_framework;
use accesskit::{Action, Role};
use anyhow::{Result, anyhow};
use block2::{self, RcBlock};
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, NSObject, Sel};
use objc2::{self, ClassType, msg_send, sel};
use objc2_core_foundation::{self, CGRect};
use objc2_foundation::{NSString, NSUUID};

use slotmap::SecondaryMap;

mod common;
mod dispatcher;
mod dynamic;
mod encoder;
mod framebuffer;
mod hid;
mod pixel_buffer;
mod reader;
mod recorder;
mod stream;
mod void_block;

pub use common::{
    BootedSimulator, ButtonDirection, Element, ElementKey, ElementTree, HardwareButton, Point,
    Rect, ScreenSpace, Screenshot, Size, TreeFilter, booted_simulators, load_frameworks,
};
pub use encoder::{
    ChunkKind, ChunkSink, EncodedChunk, EncoderConfig, H264Encoder, NalFormat, Tuning,
};
pub use framebuffer::{CapturedFrame, FrameSink, FramebufferStats, SimFramebuffer};
pub use hid::{Orientation, SimulatorHID, TouchEdge, TouchPhase};
pub use reader::IOSSimulatorAccessibility;
pub use recorder::{Recorder, Recording, RecordingConfig};
pub use stream::{ScreenGeometry, SimVideoStream};
