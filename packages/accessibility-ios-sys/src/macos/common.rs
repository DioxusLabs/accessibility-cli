use std::ffi::{CStr, c_char, c_void};

use anyhow::{Result, anyhow};
use objc2::msg_send;
use objc2::runtime::{AnyClass, AnyObject};

use accesskit::Role;
use euclid::{Point2D, Rect as EuclidRect, Size2D};
use slotmap::{Key, KeyData, SlotMap};

/// Identity of a currently booted simulator.
///
/// The UDID is the stable device identifier used to bind independent capture,
/// input, and accessibility sessions to the same simulator.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BootedSimulator {
    pub udid: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScreenSpace;

pub type Point = Point2D<f64, ScreenSpace>;
pub type Size = Size2D<f64, ScreenSpace>;
pub type Rect = EuclidRect<f64, ScreenSpace>;

slotmap::new_key_type! {
    pub struct ElementKey;
}

impl ElementKey {
    pub fn to_ffi(self) -> u64 {
        self.data().as_ffi()
    }

    pub fn from_ffi(value: u64) -> Self {
        KeyData::from_ffi(value).into()
    }
}

impl std::fmt::Display for ElementKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_ffi())
    }
}

#[derive(Debug, Clone)]
pub struct Screenshot {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl Screenshot {
    pub fn crop(&self, bounds: &Rect, screen_bounds: &Rect) -> Result<Screenshot> {
        use image::ImageReader;
        use std::io::Cursor;

        let scale_x = self.width as f64 / screen_bounds.size.width;
        let scale_y = self.height as f64 / screen_bounds.size.height;
        let px = ((bounds.origin.x - screen_bounds.origin.x) * scale_x).round() as u32;
        let py = ((bounds.origin.y - screen_bounds.origin.y) * scale_y).round() as u32;
        let pw = (bounds.size.width * scale_x).round() as u32;
        let ph = (bounds.size.height * scale_y).round() as u32;
        let px = px.min(self.width);
        let py = py.min(self.height);
        let pw = pw.min(self.width.saturating_sub(px));
        let ph = ph.min(self.height.saturating_sub(py));

        if pw == 0 || ph == 0 {
            anyhow::bail!("Crop region is empty or outside screenshot bounds");
        }

        let img = ImageReader::new(Cursor::new(&self.data))
            .with_guessed_format()?
            .decode()?;
        let cropped = img.crop_imm(px, py, pw, ph);
        let mut output = Cursor::new(Vec::new());
        cropped.write_to(&mut output, image::ImageFormat::Png)?;

        Ok(Screenshot {
            data: output.into_inner(),
            width: pw,
            height: ph,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Element {
    pub id: ElementKey,
    pub role: Role,
    pub title: Option<String>,
    pub description: Option<String>,
    pub value: Option<String>,
    pub url: Option<String>,
    pub help: Option<String>,
    pub role_description: Option<String>,
    pub identifier: Option<String>,
    pub bounds: Option<Rect>,
    pub enabled: bool,
    pub focused: bool,
    pub actions: Vec<String>,
    pub children: Vec<Element>,
}

#[derive(Debug, Clone)]
pub struct ElementTree {
    pub version: u64,
    pub pid: Option<u32>,
    pub app_name: Option<String>,
    pub root: Element,
    pub element_count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct TreeFilter {
    pub max_depth: Option<usize>,
    pub max_elements: Option<usize>,
    pub interactive_only: bool,
    pub visible_only: bool,
    pub within_bounds: Option<Rect>,
    pub roles: Option<Vec<Role>>,
}

pub(super) struct ElementCache {
    elements: SlotMap<ElementKey, Element>,
    version: u64,
}

impl ElementCache {
    pub(super) fn new() -> Self {
        Self {
            elements: SlotMap::with_key(),
            version: 0,
        }
    }

    pub(super) fn clear(&mut self) {
        self.elements.clear();
        self.version = self.version.saturating_add(1);
    }

    pub(super) fn len(&self) -> usize {
        self.elements.len()
    }

    pub(super) fn version(&self) -> u64 {
        self.version
    }

    pub(super) fn get(&self, id: ElementKey) -> Option<&Element> {
        self.elements.get(id)
    }

    pub(super) fn store_with_clone<F>(&mut self, f: F) -> (ElementKey, Element)
    where
        F: FnOnce(ElementKey) -> Element,
    {
        let mut out = None;
        let id = self.elements.insert_with_key(|id| {
            let elem = f(id);
            out = Some(elem.clone());
            elem
        });
        (id, out.expect("element should be captured"))
    }
}

fn map_ax_role(ax_role: &str) -> Role {
    let role = ax_role.strip_prefix("AX").unwrap_or(ax_role);
    match role {
        "Application" => Role::Application,
        "Window" => Role::Window,
        "Button" => Role::Button,
        "TextField" => Role::TextInput,
        "TextArea" => Role::MultilineTextInput,
        "StaticText" => Role::TextRun,
        "CheckBox" => Role::CheckBox,
        "RadioButton" => Role::RadioButton,
        "PopUpButton" | "ComboBox" => Role::ComboBox,
        "Slider" => Role::Slider,
        "Table" => Role::Table,
        "List" => Role::List,
        "Outline" => Role::Tree,
        "Sheet" => Role::Dialog,
        "Menu" => Role::Menu,
        "MenuItem" | "MenuBarItem" => Role::MenuItem,
        "MenuBar" => Role::MenuBar,
        "WebArea" => Role::WebView,
        "Group" => Role::Group,
        "Image" => Role::Image,
        "Link" => Role::Link,
        "ScrollArea" => Role::ScrollView,
        "Toolbar" => Role::Toolbar,
        "TabGroup" => Role::TabList,
        "Tab" => Role::Tab,
        "ProgressIndicator" => Role::ProgressIndicator,
        "SplitGroup" | "Splitter" => Role::Splitter,
        "Row" => Role::Row,
        "Column" => Role::ListItem,
        "Cell" => Role::Cell,
        _ => Role::Unknown,
    }
}

pub(super) fn map_ax_role_ios(ax_role: &str) -> Role {
    let role = ax_role.strip_prefix("AX").unwrap_or(ax_role);
    match role {
        "StaticText" | "Label" => Role::Label,
        "SearchField" => Role::TextInput,
        "NavigationBar" => Role::Navigation,
        "Picker" | "PickerView" => Role::ListBox,
        "Switch" | "Toggle" => Role::Switch,
        "Alert" => Role::Dialog,
        "Header" => Role::Heading,
        "WebArea" | "WebView" => Role::Document,
        "TabBar" => Role::TabList,
        "ScrollView" => Role::ScrollView,
        "TextView" => Role::MultilineTextInput,
        "Outline" => Role::Group,
        _ => map_ax_role(ax_role),
    }
}

/// Load all required private frameworks.
pub fn load_frameworks() -> Result<()> {
    crate::frameworks::load_frameworks()
}

/// Mach message header for Indigo messages.
/// Kept for documentation - we construct messages using raw byte offsets.
#[repr(C, packed(4))]
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
struct MachMessageHeader {
    msgh_bits: u32,
    msgh_size: u32,
    msgh_remote_port: u32,
    msgh_local_port: u32,
    msgh_voucher_port: u32,
    msgh_id: i32,
}

/// Touch event data in Indigo protocol.
/// Coordinates are normalized ratios (0.0 to 1.0).
/// Size: 0x70 (112 bytes)
/// Kept for documentation - we construct messages using raw byte offsets.
#[repr(C, packed(4))]
#[derive(Clone, Copy, Debug, Default)]
#[allow(dead_code)]
struct IndigoTouch {
    field1: u32,  // 0x00 - touch state flags
    field2: u32,  // 0x04 - touch state flags
    field3: u32,  // 0x08
    x_ratio: f64, // 0x0c - 0.0 = left, 1.0 = right
    y_ratio: f64, // 0x14 - 0.0 = top, 1.0 = bottom
    field6: f64,  // 0x1c
    field7: f64,  // 0x24
    field8: f64,  // 0x2c
    field9: u32,  // 0x34
    field10: u32, // 0x38
    field11: u32, // 0x3c
    field12: u32, // 0x40
    field13: u32, // 0x44
    field14: f64, // 0x48
    field15: f64, // 0x50
    field16: f64, // 0x58
    field17: f64, // 0x60
    field18: f64, // 0x68
}

/// Button event data in Indigo protocol.
#[repr(C, packed(4))]
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
struct IndigoButton {
    event_source: u32,
    event_type: u32,
    event_target: u32,
    key_code: u32,
    field5: u32,
}

/// Indigo event union - we use the largest variant (touch) for sizing.
/// The actual event type is determined by IndigoMessage.event_type.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
#[allow(dead_code)]
union IndigoEvent {
    touch: IndigoTouch,
    // button, wheel, etc. are smaller and fit within touch's space
}

impl Default for IndigoEvent {
    fn default() -> Self {
        IndigoEvent {
            touch: IndigoTouch::default(),
        }
    }
}

/// Payload embedded inside an IndigoMessage.
/// Size: 0x80 (128 bytes) - field1(4) + timestamp(8) + field3(4) + event(112)
#[repr(C, packed(4))]
#[derive(Clone, Copy, Default)]
#[allow(dead_code)]
struct IndigoPayload {
    field1: u32,        // 0x00
    timestamp: u64,     // 0x04 - mach_absolute_time
    field3: u32,        // 0x0c
    event: IndigoEvent, // 0x10
}

/// Complete Indigo message structure.
/// Base size: 0xb0 (176 bytes)
/// For touch events, we allocate extra space for duplicated payload.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct IndigoMessage {
    header: MachMessageHeader, // 0x00 - 0x18 (24 bytes)
    inner_size: u32,           // 0x18
    event_type: u8,            // 0x1c
    _padding: [u8; 3],         // 0x1d-0x1f
    payload: IndigoPayload,    // 0x20
}

/// Hardware button identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum HardwareButton {
    Home = 0x0,
    Lock = 0x1,
    ApplePay = 0x1f4,
    SideButton = 0xbb8,
    Siri = 0x400002,
}

/// Button event direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ButtonDirection {
    Down = 0x1,
    Up = 0x2,
}

// Button target constants
pub(super) const BUTTON_EVENT_TARGET_HARDWARE: u32 = 0x33;

/// Indigo event types.
#[allow(dead_code)]
const INDIGO_EVENT_TYPE_BUTTON: u8 = 1;
const INDIGO_EVENT_TYPE_TOUCH: u8 = 2;

// External function for getting mach absolute time
unsafe extern "C" {
    fn mach_absolute_time() -> u64;
}

/// Create a touch message from a template message (from IndigoHIDMessageForMouseNSEvent).
///
/// This extracts the touch payload from the template and creates a proper message
/// with duplicated payloads as required by the iOS Simulator.
pub(super) fn create_touch_message_from_template(
    template: *mut c_void,
    x_ratio: f64,
    y_ratio: f64,
    direction: ButtonDirection,
) -> *mut c_void {
    const MESSAGE_SIZE: usize = 0x140; // 320 bytes
    const PAYLOAD_STRIDE: usize = 0x80; // 128 bytes

    let message = unsafe { libc::calloc(1, MESSAGE_SIZE) as *mut u8 };
    if message.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let template_ptr = template as *mut u8;

        // Copy the header portion from template (first 0x20 bytes)
        std::ptr::copy_nonoverlapping(template_ptr, message, 0x20);

        // Set inner_size to payload stride
        std::ptr::write_unaligned(message.add(0x18) as *mut u32, PAYLOAD_STRIDE as u32);

        // Set event_type to touch
        *message.add(0x1c) = INDIGO_EVENT_TYPE_TOUCH;

        // Payload at offset 0x20
        let payload_ptr = message.add(0x20);

        // payload.field1 = 0x0b (from idb)
        std::ptr::write_unaligned(payload_ptr as *mut u32, 0x0000000bu32);

        // payload.timestamp
        std::ptr::write_unaligned(payload_ptr.add(0x04) as *mut u64, mach_absolute_time());

        // Copy the touch event data from template (at offset 0x30)
        // Touch data is 0x70 bytes
        std::ptr::copy_nonoverlapping(template_ptr.add(0x30), message.add(0x30), 0x70);

        // Patch x/y ratios
        let touch_ptr = message.add(0x30);
        std::ptr::write_unaligned(touch_ptr.add(0x0c) as *mut f64, x_ratio);
        std::ptr::write_unaligned(touch_ptr.add(0x14) as *mut f64, y_ratio);

        // Set touch state flags
        let (field1_val, field2_val) = match direction {
            ButtonDirection::Down => (0x01u32, 0x01u32),
            ButtonDirection::Up => (0x00u32, 0x00u32),
        };
        std::ptr::write_unaligned(touch_ptr as *mut u32, field1_val);
        std::ptr::write_unaligned(touch_ptr.add(0x04) as *mut u32, field2_val);

        // Duplicate the payload
        let second_payload_ptr = payload_ptr.add(PAYLOAD_STRIDE);
        std::ptr::copy_nonoverlapping(payload_ptr, second_payload_ptr, PAYLOAD_STRIDE);

        // Adjust second payload's touch fields
        let second_touch_ptr = second_payload_ptr.add(0x10);
        std::ptr::write_unaligned(second_touch_ptr as *mut u32, 0x00000001u32);
        std::ptr::write_unaligned(second_touch_ptr.add(0x04) as *mut u32, 0x00000002u32);
    }

    message as *mut c_void
}

/// Get the AXPTranslator singleton.
///
/// # Safety
/// Frameworks must be loaded first via `load_frameworks()`.
pub(super) unsafe fn get_translator() -> Result<*mut AnyObject> {
    let cls =
        AnyClass::get(c"AXPTranslator").ok_or_else(|| anyhow!("AXPTranslator class not found"))?;

    let translator: *mut AnyObject = msg_send![cls, sharedInstance];
    if translator.is_null() {
        return Err(anyhow!("Failed to get AXPTranslator sharedInstance"));
    }

    Ok(translator)
}

/// Get the default SimDeviceSet via SimServiceContext.
///
/// # Safety
/// CoreSimulator framework must be loaded.
unsafe fn get_device_set() -> Result<*mut AnyObject> {
    // Get SimServiceContext class
    let ctx_cls = AnyClass::get(c"SimServiceContext")
        .ok_or_else(|| anyhow!("SimServiceContext class not found"))?;

    // Get shared context for current developer dir (nil = use default)
    let mut error: *mut AnyObject = std::ptr::null_mut();
    let context: *mut AnyObject = msg_send![
        ctx_cls,
        sharedServiceContextForDeveloperDir: std::ptr::null::<AnyObject>(),
        error: &mut error
    ];

    if context.is_null() {
        if !error.is_null() {
            let desc: *mut AnyObject = msg_send![error, localizedDescription];
            let error_str =
                nsstring_to_string_static(desc).unwrap_or_else(|| "Unknown error".to_string());
            return Err(anyhow!("Failed to get SimServiceContext: {}", error_str));
        }
        return Err(anyhow!("Failed to get SimServiceContext: unknown error"));
    }

    // Get default device set
    let mut error: *mut AnyObject = std::ptr::null_mut();
    let device_set: *mut AnyObject = msg_send![context, defaultDeviceSetWithError: &mut error];

    if device_set.is_null() {
        if !error.is_null() {
            let desc: *mut AnyObject = msg_send![error, localizedDescription];
            let error_str =
                nsstring_to_string_static(desc).unwrap_or_else(|| "Unknown error".to_string());
            return Err(anyhow!("Failed to get default device set: {}", error_str));
        }
        return Err(anyhow!("Failed to get default device set: unknown error"));
    }

    Ok(device_set)
}

/// Convert NSString to Rust String (standalone function).
pub(super) unsafe fn nsstring_to_string_static(ns_string: *mut AnyObject) -> Option<String> {
    if ns_string.is_null() {
        return None;
    }
    let cstr: *const c_char = msg_send![ns_string, UTF8String];
    if cstr.is_null() {
        return None;
    }
    Some(CStr::from_ptr(cstr).to_string_lossy().to_string())
}

/// Enumerate all currently booted simulators.
///
/// Devices are returned in CoreSimulator's order. Each entry includes the
/// stable UDID used by the rest of this crate and the user-visible device name.
pub fn booted_simulators() -> Result<Vec<BootedSimulator>> {
    crate::frameworks::load_coresimulator_framework()?;

    unsafe {
        let device_set = get_device_set()?;

        // Resolve each device while the owning NSArray is still in scope. The
        // objects returned by objectAtIndex: are unretained and must not be
        // collected as raw pointers for later processing.
        let devices: *mut AnyObject = msg_send![device_set, devices];
        if devices.is_null() {
            return Err(anyhow!("No devices found in SimDeviceSet"));
        }

        let count: usize = msg_send![devices, count];
        let mut booted = Vec::new();
        for i in 0..count {
            let device: *mut AnyObject = msg_send![devices, objectAtIndex: i];
            if device.is_null() {
                continue;
            }

            // Check if booted (state == 3)
            let state: i64 = msg_send![device, state];
            if state != 3 {
                continue;
            }

            // A stale or partially initialized SimDevice should not hide the
            // rest of the catalog. Keep every valid entry in native order.
            if let Ok(info) = booted_simulator_info(device) {
                booted.push(info);
            }
        }
        Ok(booted)
    }
}

/// Read the stable identity fields from a SimDevice.
///
/// # Safety
/// `device` must be a live SimDevice from the loaded CoreSimulator framework.
unsafe fn booted_simulator_info(device: *mut AnyObject) -> Result<BootedSimulator> {
    let device_udid: *mut AnyObject = msg_send![device, UDID];
    if device_udid.is_null() {
        return Err(anyhow!("Booted simulator has no UDID"));
    }
    let udid_string: *mut AnyObject = msg_send![device_udid, UUIDString];
    let udid = nsstring_to_string_static(udid_string)
        .ok_or_else(|| anyhow!("Failed to read booted simulator UDID"))?;

    let name_string: *mut AnyObject = msg_send![device, name];
    let name = nsstring_to_string_static(name_string)
        .ok_or_else(|| anyhow!("Failed to read booted simulator name for {udid}"))?;

    Ok(BootedSimulator { udid, name })
}

/// Find a booted simulator device by UDID or return the first booted one.
///
/// # Safety
/// CoreSimulator framework must be loaded.
pub(super) unsafe fn find_booted_device(udid: Option<&str>) -> Result<*mut AnyObject> {
    let device_set = get_device_set()?;

    // Get all devices
    let devices: *mut AnyObject = msg_send![device_set, devices];
    if devices.is_null() {
        return Err(anyhow!("No devices found in SimDeviceSet"));
    }

    let count: usize = msg_send![devices, count];
    for i in 0..count {
        let device: *mut AnyObject = msg_send![devices, objectAtIndex: i];
        if device.is_null() {
            continue;
        }

        // Check if booted (state == 3)
        let state: i64 = msg_send![device, state];
        if state != 3 {
            continue;
        }

        // If we're looking for a specific UDID, check it. Keep malformed
        // entries skippable just as the original lookup did.
        if let Some(target_udid) = udid {
            let device_udid: *mut AnyObject = msg_send![device, UDID];
            if device_udid.is_null() {
                continue;
            }
            let udid_string: *mut AnyObject = msg_send![device_udid, UUIDString];
            let Some(device_udid) = nsstring_to_string_static(udid_string) else {
                continue;
            };
            if device_udid == target_udid {
                return Ok(device);
            }
        } else {
            // Return first booted device
            return Ok(device);
        }
    }

    if let Some(target_udid) = udid {
        Err(anyhow!(
            "No booted simulator found with UDID: {}",
            target_udid
        ))
    } else {
        Err(anyhow!("No booted simulator found"))
    }
}
