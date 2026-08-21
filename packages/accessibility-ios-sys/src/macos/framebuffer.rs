//! Live framebuffer capture for a booted iOS Simulator.
//!
//! SimulatorKit exposes the simulator's display as an `IOSurface` behind a
//! private "device IO port" graph. This module walks that graph and registers
//! screen callbacks so we get pushed a notification whenever the display is
//! repainted, which is dramatically cheaper and lower latency than polling
//! `xcrun simctl io screenshot`.
//!
//! The traversal is:
//!
//! ```text
//! SimDevice -> [device io]        (SimDeviceIOClient)
//!           -> updateIOPorts
//!           -> deviceIOPorts      (filter portIdentifier == com.apple.framebuffer.display)
//!           -> [port descriptor]
//!           -> registerScreenCallbacksWithUUID:...
//!           -> [descriptor framebufferSurface] -> IOSurface
//! ```
//!
//! Registering the callbacks is load-bearing: it is what makes SimulatorKit
//! attach the display pipeline to our client and populate `framebufferSurface`.
//! Merely reading the property without registering does not reliably work.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, sel};
use objc2_core_foundation::{CFRetained, kCFAllocatorDefault};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferCreateWithIOSurface, CVPixelBufferGetHeight, CVPixelBufferGetWidth,
    kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{NSString, NSUUID};
use objc2_io_surface::IOSurfaceRef;

use super::common::find_booted_device;
use super::dynamic::{
    responds_to, send_id, send_id_with_id, send_register_screen_callbacks, send_u16,
    send_void_with_id,
};
use super::pixel_buffer::cf_dict_u32;
use super::void_block::VoidBlock;

/// Port identifier for the simulator's main display framebuffer.
///
/// Several ports can carry this identifier (main screen plus secondary planes),
/// so every match is registered and the largest live surface wins.
const FRAMEBUFFER_PORT_ID: &str = "com.apple.framebuffer.display";

/// How often to force a repaint when the screen is otherwise idle.
///
/// This is not an optimization. A `multipart/x-mixed-replace` consumer will not
/// paint a part until the *next* boundary arrives, and clients that join while
/// the simulator is static would otherwise sit blank forever. ~5fps of forced
/// re-emits keeps both honest.
const IDLE_INTERVAL: Duration = Duration::from_millis(200);

/// Keep polling briefly after input stops so animations and inertial scrolling
/// do not fall back to the idle cadence between input events.
const INTERACTION_HOLD: Duration = Duration::from_secs(1);

/// How long to wait between re-wire attempts while no frame has ever been
/// captured.
///
/// Descriptors are sometimes created lazily, so a registration that happened
/// too early silently yields nothing. Rebuilding the port graph roughly once a
/// second recovers from that.
const REWIRE_INTERVAL: Duration = Duration::from_secs(1);

/// A frame handed to the sink, valid only for the duration of the call.
///
/// The pixel buffer wraps SimulatorKit's live framebuffer surface, which is
/// recycled in place. Anything retaining it past the call must copy first.
pub struct CapturedFrame<'a> {
    pub pixel_buffer: &'a CVPixelBuffer,
    pub width: u32,
    pub height: u32,
    pub captured_at: Instant,
}

/// Sink invoked on the capture queue for every accepted frame.
pub type FrameSink = Box<dyn FnMut(CapturedFrame<'_>) + Send + 'static>;

#[derive(Debug, Clone, Default)]
pub struct FramebufferStats {
    pub frame_count: u64,
    pub width: u32,
    pub height: u32,
    pub descriptor_count: usize,
    pub rewire_count: u64,
    /// `displayClass` of each registered descriptor, for diagnostics.
    pub display_classes: Vec<Option<u16>>,
}

/// Wrapper making a raw Objective-C pointer movable across threads.
///
/// The pointers here are owned by SimulatorKit and only ever messaged from the
/// capture queue (or from `start`/`stop`, which are externally serialized).
#[derive(Clone, Copy)]
struct ObjcPtr(*mut AnyObject);
unsafe impl Send for ObjcPtr {}
unsafe impl Sync for ObjcPtr {}

impl ObjcPtr {
    fn as_ptr(self) -> *mut AnyObject {
        self.0
    }
}

/// A framebuffer descriptor plus the UUID its callbacks were registered under.
struct Registration {
    descriptor: ObjcPtr,
    uuid: Retained<NSUUID>,
    /// Last `IOSurfaceGetSeed` observed, used to skip unchanged repaints.
    last_seed: Option<u32>,
    /// `displayClass` from the descriptor's state. iOS reports the main
    /// display as 0; anything else is a secondary display or plane.
    display_class: Option<u16>,
}

/// State shared between the capture queue, the idle timer, and the owner.
///
/// The wiring state (device, IO client, registrations, blocks) lives here
/// rather than on [`SimFramebuffer`] so the idle timer can rebuild the whole
/// pipeline without needing `&mut` access to the owner.
struct CaptureState {
    device: ObjcPtr,
    queue: DispatchRetained<DispatchQueue>,
    registrations: Mutex<Vec<Registration>>,
    /// SimulatorKit retains these blocks for the lifetime of the registration.
    /// Dropping them early is a use-after-free, not a clean failure, so they
    /// are held until the matching unregister call.
    blocks: Mutex<Vec<VoidBlock>>,
    sink: Mutex<Option<FrameSink>>,
    last_capture: Mutex<Instant>,
    active_until: Mutex<Instant>,
    activity: Condvar,
    active_interval_micros: AtomicU64,
    frame_count: AtomicU64,
    rewire_count: AtomicU64,
    width: AtomicUsize,
    height: AtomicUsize,
    running: AtomicBool,
}

impl CaptureState {
    fn stats(&self) -> FramebufferStats {
        // Lock once and finish with it. Two `lock()` calls inside one struct
        // expression would deadlock: the temporary guard from the first lives
        // until the end of the whole expression, so the second waits on it.
        let registrations = self.registrations.lock().unwrap();
        let descriptor_count = registrations.len();
        let display_classes = registrations
            .iter()
            .map(|registration| registration.display_class)
            .collect();
        drop(registrations);

        FramebufferStats {
            frame_count: self.frame_count.load(Ordering::Relaxed),
            width: self.width.load(Ordering::Relaxed) as u32,
            height: self.height.load(Ordering::Relaxed) as u32,
            descriptor_count,
            rewire_count: self.rewire_count.load(Ordering::Relaxed),
            display_classes,
        }
    }

    /// Choose which descriptor to capture from.
    ///
    /// iOS reports the main display as `displayClass == 0`, so prefer that.
    /// Several descriptors share the framebuffer port identifier — secondary
    /// displays and planes — and the class is the value the API actually
    /// provides for telling them apart. Largest live area remains the
    /// fallback for descriptors that do not report a class, and for platforms
    /// like tvOS that render on a non-zero class.
    fn pick_best_surface(&self) -> Option<(usize, CFRetained<IOSurfaceRef>)> {
        let registrations = self.registrations.lock().unwrap();
        let mut best: Option<(usize, CFRetained<IOSurfaceRef>, usize)> = None;

        for (index, registration) in registrations.iter().enumerate() {
            let Some(surface) = (unsafe { framebuffer_surface(registration.descriptor.as_ptr()) })
            else {
                continue;
            };
            let area = surface.width() * surface.height();
            if area == 0 {
                continue;
            }
            if registration.display_class == Some(0) {
                return Some((index, surface));
            }
            if best
                .as_ref()
                .is_none_or(|(_, _, best_area)| area > *best_area)
            {
                best = Some((index, surface, area));
            }
        }

        best.map(|(index, surface, _)| (index, surface))
    }

    /// Capture one frame. `force` bypasses the unchanged-seed check.
    fn capture(&self, force: bool) {
        if !self.running.load(Ordering::Relaxed) {
            return;
        }

        let Some((index, surface)) = self.pick_best_surface() else {
            return;
        };

        // Skip repaints of an unchanged surface, but never skip the very first
        // frame and never skip a forced idle re-emit.
        let seed = surface.seed();
        {
            let mut registrations = self.registrations.lock().unwrap();
            let Some(registration) = registrations.get_mut(index) else {
                return;
            };
            let unchanged = registration.last_seed == Some(seed);
            if unchanged && !force && self.frame_count.load(Ordering::Relaxed) > 0 {
                return;
            }
            registration.last_seed = Some(seed);
        }

        let width = surface.width();
        let height = surface.height();
        self.width.store(width, Ordering::Relaxed);
        self.height.store(height, Ordering::Relaxed);

        let Some(live) = (unsafe { pixel_buffer_for_surface(&surface) }) else {
            return;
        };

        let captured_at = Instant::now();
        *self.last_capture.lock().unwrap() = captured_at;

        self.frame_count.fetch_add(1, Ordering::Relaxed);

        // Handed out live. SimulatorKit recycles this surface in place, so the
        // sink must finish with it — or take its own copy — before returning;
        // the encoder's pixel transfer is what does that.
        let mut sink = self.sink.lock().unwrap();
        if let Some(sink) = sink.as_mut() {
            sink(CapturedFrame {
                pixel_buffer: &live,
                width: CVPixelBufferGetWidth(&live) as u32,
                height: CVPixelBufferGetHeight(&live) as u32,
                captured_at,
            });
        }
    }

    /// How long the idle thread should wait between forced re-emits: the
    /// configured capture cadence while interaction is still recent, the idle
    /// heartbeat once it has lapsed.
    fn tick_interval(&self, active_until: Instant) -> Duration {
        if active_until > Instant::now() {
            Duration::from_micros(self.active_interval_micros.load(Ordering::Relaxed))
        } else {
            IDLE_INTERVAL
        }
    }

    /// Walk the IO port graph and register screen callbacks on every
    /// framebuffer descriptor, replacing any existing registration.
    fn wire_up(self: &Arc<Self>) -> Result<()> {
        let io: *mut AnyObject = unsafe { msg_send![self.device.as_ptr(), io] };
        if io.is_null() {
            return Err(anyhow!(
                "SimDevice returned no IO client; is the simulator still booted?"
            ));
        }

        let _: () = unsafe { msg_send![io, updateIOPorts] };

        let descriptors = unsafe { framebuffer_descriptors(io)? };
        if descriptors.is_empty() {
            return Err(anyhow!(
                "No '{FRAMEBUFFER_PORT_ID}' IO ports found on the simulator"
            ));
        }

        self.unregister_all();

        let mut registrations = Vec::with_capacity(descriptors.len());
        let mut blocks = Vec::with_capacity(descriptors.len() * 3);

        for descriptor in descriptors {
            let uuid = NSUUID::new();

            let state = Arc::clone(self);
            let frame_cb = VoidBlock::new(move || state.capture(false));
            // `surfacesChanged` carries no payload; it just means "re-query".
            let state = Arc::clone(self);
            let surfaces_cb = VoidBlock::new(move || state.capture(true));
            let props_cb = VoidBlock::new(|| {});

            unsafe {
                send_register_screen_callbacks(
                    descriptor,
                    sel!(registerScreenCallbacksWithUUID:callbackQueue:frameCallback:surfacesChangedCallback:propertiesChangedCallback:),
                    &*uuid as *const NSUUID as *mut AnyObject,
                    &*self.queue as *const DispatchQueue as *mut c_void,
                    frame_cb.as_ptr(),
                    surfaces_cb.as_ptr(),
                    props_cb.as_ptr(),
                );
            }

            blocks.push(frame_cb);
            blocks.push(surfaces_cb);
            blocks.push(props_cb);
            registrations.push(Registration {
                descriptor: ObjcPtr(descriptor),
                uuid,
                last_seed: None,
                display_class: unsafe { display_class(descriptor) },
            });
        }

        *self.registrations.lock().unwrap() = registrations;
        *self.blocks.lock().unwrap() = blocks;

        // Registration alone does not deliver a first frame; prime it.
        self.capture(true);
        Ok(())
    }

    fn unregister_all(&self) {
        let mut registrations = self.registrations.lock().unwrap();
        for registration in registrations.drain(..) {
            let descriptor = registration.descriptor.as_ptr();
            let selector = sel!(unregisterScreenCallbacksWithUUID:);
            if unsafe { responds_to(descriptor, selector) } {
                unsafe {
                    send_void_with_id(
                        descriptor,
                        selector,
                        &*registration.uuid as *const NSUUID as *mut AnyObject,
                    );
                }
            }
        }
        drop(registrations);
        // Only safe to release the blocks once SimulatorKit has been told to
        // stop calling them.
        self.blocks.lock().unwrap().clear();
        self.width.store(0, Ordering::Relaxed);
        self.height.store(0, Ordering::Relaxed);
    }
}

/// Live framebuffer capture session for one simulator device.
pub struct SimFramebuffer {
    device_udid: String,
    state: Arc<CaptureState>,
    idle_thread: Option<std::thread::JoinHandle<()>>,
}

impl SimFramebuffer {
    /// Attach to a booted simulator. `udid` of `None` picks the first booted one.
    pub fn new(udid: Option<&str>) -> Result<Self> {
        crate::frameworks::load_coresimulator_framework()?;
        crate::frameworks::load_simulatorkit_framework()?;

        let device = unsafe { find_booted_device(udid)? };
        let device_udid = unsafe { device_udid_string(device)? };

        Ok(Self {
            device_udid,
            state: Arc::new(CaptureState {
                device: ObjcPtr(device),
                queue: DispatchQueue::new(
                    "com.accessibility_cli.framebuffer",
                    DispatchQueueAttr::SERIAL,
                ),
                registrations: Mutex::new(Vec::new()),
                blocks: Mutex::new(Vec::new()),
                sink: Mutex::new(None),
                last_capture: Mutex::new(Instant::now()),
                active_until: Mutex::new(Instant::now()),
                activity: Condvar::new(),
                active_interval_micros: AtomicU64::new(1_000_000 / 60),
                frame_count: AtomicU64::new(0),
                rewire_count: AtomicU64::new(0),
                width: AtomicUsize::new(0),
                height: AtomicUsize::new(0),
                running: AtomicBool::new(false),
            }),
            idle_thread: None,
        })
    }

    pub fn device_udid(&self) -> &str {
        &self.device_udid
    }

    pub fn stats(&self) -> FramebufferStats {
        self.state.stats()
    }

    /// Install the frame sink. Called on the capture queue, so it must be quick.
    pub fn set_sink(&mut self, sink: Option<FrameSink>) {
        *self.state.sink.lock().unwrap() = sink;
    }

    pub fn set_active_frame_rate(&self, fps: u32) {
        self.state
            .active_interval_micros
            .store(1_000_000 / u64::from(fps.max(1)), Ordering::Relaxed);
    }

    pub fn note_interaction(&self) {
        *self.state.active_until.lock().unwrap() = Instant::now() + INTERACTION_HOLD;
        self.state.activity.notify_one();
    }

    /// Begin capturing. Calling twice rebuilds the pipeline.
    pub fn start(&mut self) -> Result<()> {
        self.state.running.store(true, Ordering::Relaxed);
        self.state.wire_up()?;
        self.start_idle_timer();
        Ok(())
    }

    /// Drive forced re-emits, and rebuild the pipeline while nothing arrives.
    fn start_idle_timer(&mut self) {
        if self.idle_thread.is_some() {
            return;
        }
        let state = Arc::clone(&self.state);
        self.idle_thread = Some(std::thread::spawn(move || {
            let mut next_rewire = Instant::now() + REWIRE_INTERVAL;
            while state.running.load(Ordering::Relaxed) {
                let active_until = state.active_until.lock().unwrap();
                let interval = state.tick_interval(*active_until);
                let (active_until, _) =
                    state.activity.wait_timeout(active_until, interval).unwrap();
                drop(active_until);
                if !state.running.load(Ordering::Relaxed) {
                    break;
                }

                // Re-read after the wait: an interaction may have arrived while
                // the thread was parked, which shortens the interval.
                let interval = state.tick_interval(*state.active_until.lock().unwrap());
                let idle_for = state.last_capture.lock().unwrap().elapsed();
                if idle_for >= interval {
                    state.capture(true);
                }

                // Self-heal only until the first frame ever lands. After that
                // a silent pipeline means an idle screen, not a broken graph.
                if state.frame_count.load(Ordering::Relaxed) == 0 && Instant::now() >= next_rewire {
                    next_rewire = Instant::now() + REWIRE_INTERVAL;
                    state.rewire_count.fetch_add(1, Ordering::Relaxed);
                    // Descriptors are sometimes created lazily, so a
                    // registration that happened too early yields nothing.
                    // Failures are expected here; the next tick retries.
                    let _ = state.wire_up();
                }
            }
        }));
    }

    pub fn stop(&mut self) {
        self.state.running.store(false, Ordering::Relaxed);
        self.state.activity.notify_one();
        if let Some(handle) = self.idle_thread.take() {
            let _ = handle.join();
        }
        self.state.unregister_all();
        *self.state.sink.lock().unwrap() = None;
    }
}

impl Drop for SimFramebuffer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Read `-framebufferSurface` off a descriptor, retaining the result.
unsafe fn framebuffer_surface(descriptor: *mut AnyObject) -> Option<CFRetained<IOSurfaceRef>> {
    if descriptor.is_null() {
        return None;
    }
    let selector = sel!(framebufferSurface);
    if !unsafe { responds_to(descriptor, selector) } {
        return None;
    }
    let surface = unsafe { send_id(descriptor, selector) };
    let surface = NonNull::new(surface)?.cast::<IOSurfaceRef>();
    // The getter returns an autoreleased/unowned surface; retain it so it
    // survives for as long as we hold it.
    Some(unsafe { CFRetained::retain(surface) })
}

/// Wrap an IOSurface as a BGRA `CVPixelBuffer` without copying pixels.
unsafe fn pixel_buffer_for_surface(surface: &IOSurfaceRef) -> Option<CFRetained<CVPixelBuffer>> {
    let attrs = unsafe {
        cf_dict_u32(
            objc2_core_video::kCVPixelBufferPixelFormatTypeKey,
            kCVPixelFormatType_32BGRA,
        )
    };

    let mut out: *mut CVPixelBuffer = std::ptr::null_mut();
    let status = unsafe {
        CVPixelBufferCreateWithIOSurface(
            kCFAllocatorDefault,
            surface,
            Some(&attrs),
            NonNull::from(&mut out),
        )
    };
    if status != 0 {
        return None;
    }
    NonNull::new(out).map(|p| unsafe { CFRetained::from_raw(p) })
}

/// Collect the descriptors of every `com.apple.framebuffer.display` IO port.
///
/// Ports arrive as `ROCKRemoteProxy` objects that implement their interface by
/// forwarding, so every message has to go through the dynamic helpers rather
/// than `msg_send!`.
unsafe fn framebuffer_descriptors(io: *mut AnyObject) -> Result<Vec<*mut AnyObject>> {
    let key = NSString::from_str("deviceIOPorts");
    let ports = unsafe {
        send_id_with_id(
            io,
            sel!(valueForKey:),
            &*key as *const NSString as *mut AnyObject,
        )
    };
    if ports.is_null() {
        return Err(anyhow!("SimDeviceIOClient exposed no deviceIOPorts"));
    }

    let count: usize = unsafe { msg_send![ports, count] };
    let mut descriptors = Vec::new();

    for index in 0..count {
        let port: *mut AnyObject = unsafe { msg_send![ports, objectAtIndex: index] };

        if !unsafe { responds_to(port, sel!(portIdentifier)) } {
            continue;
        }
        let identifier = unsafe { send_id(port, sel!(portIdentifier)) };
        // The identifier is usually an NSString but can be a richer object;
        // falling back to -description matches serve-sim's stringification.
        let Some(identifier) = (unsafe { super::common::nsstring_to_string_static(identifier) })
            .or_else(|| unsafe { object_description(identifier) })
        else {
            continue;
        };
        if identifier != FRAMEBUFFER_PORT_ID {
            continue;
        }

        if !unsafe { responds_to(port, sel!(descriptor)) } {
            continue;
        }
        let descriptor = unsafe { send_id(port, sel!(descriptor)) };

        // Not every descriptor on a framebuffer port actually vends a surface.
        if !unsafe { responds_to(descriptor, sel!(framebufferSurface)) } {
            continue;
        }

        descriptors.push(descriptor);
    }

    Ok(descriptors)
}

/// Read `displayClass` from a descriptor's state, if it reports one.
///
/// The state object is another proxied private object, so both hops go
/// through dynamic dispatch.
unsafe fn display_class(descriptor: *mut AnyObject) -> Option<u16> {
    let state_selector = sel!(state);
    if !unsafe { responds_to(descriptor, state_selector) } {
        return None;
    }
    let state = unsafe { send_id(descriptor, state_selector) };
    let class_selector = sel!(displayClass);
    if !unsafe { responds_to(state, class_selector) } {
        return None;
    }
    Some(unsafe { send_u16(state, class_selector) })
}

/// `-[NSObject description]` as a Rust string, for non-NSString identifiers.
unsafe fn object_description(object: *mut AnyObject) -> Option<String> {
    if !unsafe { responds_to(object, sel!(description)) } {
        return None;
    }
    unsafe { super::common::nsstring_to_string_static(send_id(object, sel!(description))) }
}

unsafe fn device_udid_string(device: *mut AnyObject) -> Result<String> {
    let udid: *mut AnyObject = unsafe { msg_send![device, UDID] };
    let string: *mut AnyObject = unsafe { msg_send![udid, UUIDString] };
    unsafe { super::common::nsstring_to_string_static(string) }
        .ok_or_else(|| anyhow!("Failed to read simulator UDID"))
}
