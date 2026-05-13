use super::symbols::ax_ui_element_get_window;
use super::{AxErrorCode, Point, Rect, Size, WindowId};
use objc2_application_services::{AXError, AXObserver, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFIndex, CFNumber, CFRetained, CFRunLoop, CFRunLoopMode,
    CFRunLoopSource, CFString, CFType, kCFRunLoopDefaultMode,
};
use objc2_core_graphics::CGWindowID;
use std::ffi::c_void;
use std::fmt;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};

const DEFAULT_MESSAGING_TIMEOUT_SECONDS: f32 = 1.0;
const AX_UI_ELEMENTS_FOR_SEARCH_PREDICATE: &str = "AXUIElementsForSearchPredicate";
const AX_SEARCH_KEY: &str = "AXSearchKey";
const AX_RESULTS_LIMIT: &str = "AXResultsLimit";
const AX_DIRECTION: &str = "AXDirection";
const AX_DIRECTION_NEXT: &str = "AXDirectionNext";
const AX_DIRECTION_PREVIOUS: &str = "AXDirectionPrevious";

pub const AX_SEARCH_KEY_BUTTON: &str = "AXButtonSearchKey";
pub const AX_SEARCH_KEY_CHECKBOX: &str = "AXCheckBoxSearchKey";
pub const AX_SEARCH_KEY_CONTROL: &str = "AXControlSearchKey";
pub const AX_SEARCH_KEY_GRAPHIC: &str = "AXGraphicSearchKey";
pub const AX_SEARCH_KEY_HEADING: &str = "AXHeadingSearchKey";
pub const AX_SEARCH_KEY_LINK: &str = "AXLinkSearchKey";
pub const AX_SEARCH_KEY_LIST: &str = "AXListSearchKey";
pub const AX_SEARCH_KEY_RADIO_GROUP: &str = "AXRadioGroupSearchKey";
pub const AX_SEARCH_KEY_STATIC_TEXT: &str = "AXStaticTextSearchKey";
pub const AX_SEARCH_KEY_TABLE: &str = "AXTableSearchKey";
pub const AX_SEARCH_KEY_TEXT_FIELD: &str = "AXTextFieldSearchKey";

#[derive(Clone)]
pub struct AxElement {
    inner: CFRetained<AXUIElement>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxSearchDirection {
    Next,
    Previous,
}

impl AxSearchDirection {
    fn as_ax_value(self) -> &'static str {
        match self {
            Self::Next => AX_DIRECTION_NEXT,
            Self::Previous => AX_DIRECTION_PREVIOUS,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AxSearchPredicate<'a> {
    pub keys: &'a [&'a str],
    pub limit: i32,
    pub direction: AxSearchDirection,
}

impl<'a> AxSearchPredicate<'a> {
    pub fn new(keys: &'a [&'a str], limit: i32) -> Self {
        Self {
            keys,
            limit,
            direction: AxSearchDirection::Next,
        }
    }
}

// AXUIElementRef is an opaque Core Foundation handle to a remote accessibility
// object. The underlying objc2 binding is conservatively !Send, but the AX
// calls we expose are synchronous process-bound IPC and do not rely on AppKit
// thread affinity. We move handles between the async caller and a blocking AX
// worker thread, never share mutable wrapper state concurrently.
unsafe impl Send for AxElement {}

#[derive(Clone)]
pub struct AxObserver {
    inner: CFRetained<AXObserver>,
}

#[derive(Clone)]
pub struct RunLoop {
    inner: CFRetained<CFRunLoop>,
}

#[derive(Clone)]
pub struct RunLoopSource {
    inner: CFRetained<CFRunLoopSource>,
}

impl fmt::Debug for AxElement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AxElement")
            .field("identity", &self.identity())
            .finish()
    }
}

unsafe extern "C-unwind" fn ax_notification_callback(
    _observer: NonNull<AXObserver>,
    _element: NonNull<AXUIElement>,
    _notification: NonNull<CFString>,
    refcon: *mut c_void,
) {
    if let Some(notified) = unsafe { (refcon as *const AtomicBool).as_ref() } {
        notified.store(true, Ordering::SeqCst);
    }
}

fn default_run_loop_mode() -> Option<&'static CFRunLoopMode> {
    unsafe { kCFRunLoopDefaultMode }
}

impl AxElement {
    fn new(inner: CFRetained<AXUIElement>) -> Self {
        let element = Self { inner };
        let _ = element.set_messaging_timeout(DEFAULT_MESSAGING_TIMEOUT_SECONDS);
        element
    }

    pub fn system_wide() -> Self {
        Self::new(unsafe { AXUIElement::new_system_wide() })
    }

    pub fn application(pid: u32) -> Self {
        Self::new(unsafe { AXUIElement::new_application(pid as libc::pid_t) })
    }

    pub fn set_messaging_timeout(&self, seconds: f32) -> std::result::Result<(), AxErrorCode> {
        ax_result(unsafe { self.inner.set_messaging_timeout(seconds.max(0.0)) })
    }

    pub fn identity(&self) -> usize {
        self.inner.as_ref() as *const AXUIElement as usize
    }

    pub fn pid(&self) -> Option<u32> {
        let mut pid: libc::pid_t = 0;
        let pid_ptr = NonNull::new(&mut pid as *mut libc::pid_t)?;
        let result = unsafe { self.inner.pid(pid_ptr) };
        if result == AXError::Success && pid > 0 {
            Some(pid as u32)
        } else {
            None
        }
    }

    pub fn attribute_names(&self) -> Vec<String> {
        let mut names: *const CFArray = std::ptr::null();
        let result = unsafe {
            self.inner
                .copy_attribute_names(NonNull::new(&mut names).unwrap())
        };
        if result != AXError::Success || names.is_null() {
            return Vec::new();
        }

        let names = NonNull::new(names as *mut CFArray as *mut CFArray<CFString>).unwrap();
        let array: CFRetained<CFArray<CFString>> = unsafe { CFRetained::from_raw(names) };

        (0..array.len())
            .filter_map(|i| array.get(i).map(|name| name.to_string()))
            .collect()
    }

    pub fn has_attribute(&self, attribute: &str) -> bool {
        self.attribute_names().iter().any(|name| name == attribute)
    }

    pub fn parameterized_attribute_names(&self) -> Vec<String> {
        let mut names: *const CFArray = std::ptr::null();
        let result = unsafe {
            self.inner
                .copy_parameterized_attribute_names(NonNull::new(&mut names).unwrap())
        };
        if result != AXError::Success || names.is_null() {
            return Vec::new();
        }

        let names = NonNull::new(names as *mut CFArray as *mut CFArray<CFString>).unwrap();
        let array: CFRetained<CFArray<CFString>> = unsafe { CFRetained::from_raw(names) };

        (0..array.len())
            .filter_map(|i| array.get(i).map(|name| name.to_string()))
            .collect()
    }

    pub fn has_parameterized_attribute(&self, attribute: &str) -> bool {
        self.parameterized_attribute_names()
            .iter()
            .any(|name| name == attribute)
    }

    pub fn supports_ui_elements_for_search_predicate(&self) -> bool {
        self.has_parameterized_attribute(AX_UI_ELEMENTS_FOR_SEARCH_PREDICATE)
    }

    pub fn attribute_string(&self, attribute: &str) -> Option<String> {
        self.copy_attribute_value(attribute)
            .ok()
            .and_then(|value| value.downcast::<CFString>().ok())
            .map(|value| value.to_string())
    }

    pub fn attribute_bool(&self, attribute: &str) -> Option<bool> {
        let value = self.copy_attribute_value(attribute).ok()?;
        match value.downcast::<CFBoolean>() {
            Ok(value) => Some(value.value()),
            Err(_) => Some(true),
        }
    }

    pub fn attribute_point(&self, attribute: &str) -> Option<Point> {
        let value = self.copy_attribute_value(attribute).ok()?;
        let ax_value = value.downcast_ref::<AXValue>()?;

        let mut point = objc2_core_foundation::CGPoint { x: 0.0, y: 0.0 };
        let success = unsafe {
            ax_value.value(
                AXValueType::CGPoint,
                NonNull::new(&mut point as *mut _ as *mut _).unwrap(),
            )
        };

        success.then_some(Point::new(point.x, point.y))
    }

    pub fn attribute_size(&self, attribute: &str) -> Option<Size> {
        let value = self.copy_attribute_value(attribute).ok()?;
        let ax_value = value.downcast_ref::<AXValue>()?;

        let mut size = objc2_core_foundation::CGSize {
            width: 0.0,
            height: 0.0,
        };
        let success = unsafe {
            ax_value.value(
                AXValueType::CGSize,
                NonNull::new(&mut size as *mut _ as *mut _).unwrap(),
            )
        };

        success.then_some(Size::new(size.width, size.height))
    }

    pub fn bounds(&self, position_attribute: &str, size_attribute: &str) -> Option<Rect> {
        Some(Rect::new(
            self.attribute_point(position_attribute)?,
            self.attribute_size(size_attribute)?,
        ))
    }

    pub fn attribute_elements(&self, attribute: &str) -> Vec<AxElement> {
        let mut elements = self.array_attribute_values(attribute);

        let value = match self.copy_attribute_value(attribute) {
            Ok(value) => value,
            Err(_) => return elements,
        };

        match value.downcast::<CFArray>() {
            Ok(array) => {
                let array: CFRetained<CFArray<AXUIElement>> =
                    unsafe { CFRetained::cast_unchecked(array) };
                for i in 0..array.len() {
                    if let Some(element) = array.get(i) {
                        elements.push(Self::new(element));
                    }
                }
            }
            Err(value) => {
                if let Ok(element) = value.downcast::<AXUIElement>() {
                    elements.push(Self::new(element));
                }
            }
        }

        elements
    }

    pub fn ui_elements_for_search_predicate(
        &self,
        predicate: AxSearchPredicate<'_>,
    ) -> Vec<AxElement> {
        if predicate.keys.is_empty() {
            return Vec::new();
        }

        let search_key_values: Vec<CFRetained<CFString>> = predicate
            .keys
            .iter()
            .map(|key| CFString::from_str(key))
            .collect();
        let search_key_refs: Vec<&CFString> =
            search_key_values.iter().map(|key| key.as_ref()).collect();
        let identifiers = CFArray::from_objects(&search_key_refs);

        let search_key = CFString::from_str(AX_SEARCH_KEY);
        let limit_key = CFString::from_str(AX_RESULTS_LIMIT);
        let direction_key = CFString::from_str(AX_DIRECTION);
        let direction_value = CFString::from_str(predicate.direction.as_ax_value());
        let limit_value = CFNumber::new_i32(predicate.limit.max(1));

        let keys: [&CFString; 3] = [&search_key, &limit_key, &direction_key];
        let identifiers_value: &CFType = identifiers.as_ref();
        let limit_value: &CFType = limit_value.as_ref();
        let direction_value: &CFType = direction_value.as_ref();
        let values: [&CFType; 3] = [identifiers_value, limit_value, direction_value];
        let predicate = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);

        let value = match self.copy_parameterized_attribute_value(
            AX_UI_ELEMENTS_FOR_SEARCH_PREDICATE,
            predicate.as_ref(),
        ) {
            Ok(value) => value,
            Err(_) => return Vec::new(),
        };

        let Ok(array) = value.downcast::<CFArray>() else {
            return Vec::new();
        };

        let array: CFRetained<CFArray<AXUIElement>> = unsafe { CFRetained::cast_unchecked(array) };
        (0..array.len())
            .filter_map(|i| array.get(i).map(Self::new))
            .collect()
    }

    pub fn action_names(&self) -> Vec<String> {
        let mut names: *const CFArray = std::ptr::null();
        let result = unsafe {
            self.inner
                .copy_action_names(NonNull::new(&mut names).unwrap())
        };
        if result != AXError::Success || names.is_null() {
            return Vec::new();
        }

        let names = NonNull::new(names as *mut CFArray as *mut CFArray<CFString>).unwrap();
        let array: CFRetained<CFArray<CFString>> = unsafe { CFRetained::from_raw(names) };

        (0..array.len())
            .filter_map(|i| array.get(i).map(|name| name.to_string()))
            .collect()
    }

    pub fn set_bool_attribute(&self, attribute: &str, enabled: bool) -> bool {
        self.set_bool_attribute_result(attribute, enabled)
            .is_success()
    }

    pub fn set_bool_attribute_result(&self, attribute: &str, enabled: bool) -> AxErrorCode {
        let attr = CFString::from_str(attribute);
        let value = CFBoolean::new(enabled);
        AxErrorCode::from_ax_error(unsafe { self.inner.set_attribute_value(&attr, value.as_ref()) })
    }

    pub fn set_string_attribute(
        &self,
        attribute: &str,
        value: &str,
    ) -> std::result::Result<(), AxErrorCode> {
        let attr = CFString::from_str(attribute);
        let value = CFString::from_str(value);
        ax_result(unsafe { self.inner.set_attribute_value(&attr, &value) })
    }

    pub fn set_point_attribute(
        &self,
        attribute: &str,
        point: Point,
    ) -> std::result::Result<(), AxErrorCode> {
        let attr = CFString::from_str(attribute);
        let mut point = objc2_core_foundation::CGPoint {
            x: point.x,
            y: point.y,
        };
        let Some(value) = (unsafe {
            AXValue::new(
                AXValueType::CGPoint,
                NonNull::new(&mut point as *mut _ as *mut c_void).unwrap(),
            )
        }) else {
            return Err(AxErrorCode::FAILURE);
        };

        ax_result(unsafe { self.inner.set_attribute_value(&attr, value.as_ref()) })
    }

    pub fn set_size_attribute(
        &self,
        attribute: &str,
        size: Size,
    ) -> std::result::Result<(), AxErrorCode> {
        let attr = CFString::from_str(attribute);
        let mut size = objc2_core_foundation::CGSize {
            width: size.width,
            height: size.height,
        };
        let Some(value) = (unsafe {
            AXValue::new(
                AXValueType::CGSize,
                NonNull::new(&mut size as *mut _ as *mut c_void).unwrap(),
            )
        }) else {
            return Err(AxErrorCode::FAILURE);
        };

        ax_result(unsafe { self.inner.set_attribute_value(&attr, value.as_ref()) })
    }

    pub fn perform_action(&self, action: &str) -> std::result::Result<(), AxErrorCode> {
        let action = CFString::from_str(action);
        ax_result(unsafe { self.inner.perform_action(&action) })
    }

    pub fn window_id(&self) -> Option<WindowId> {
        let get_window = ax_ui_element_get_window()?;
        let mut window_id: CGWindowID = 0;
        let result = unsafe { get_window(&self.inner, &mut window_id) };
        if result == AXError::Success && window_id != 0 {
            Some(WindowId(window_id))
        } else {
            None
        }
    }

    pub fn element_at_position(&self, x: f64, y: f64) -> Option<AxElement> {
        let mut element: *const AXUIElement = std::ptr::null();
        let element_ptr: *mut *const AXUIElement = &mut element;
        let result = unsafe {
            self.inner.copy_element_at_position(
                x as f32,
                y as f32,
                NonNull::new(element_ptr).unwrap(),
            )
        };

        if result != AXError::Success || element.is_null() {
            None
        } else {
            let ptr = NonNull::new(element as *mut AXUIElement).unwrap();
            Some(Self::new(unsafe { CFRetained::from_raw(ptr) }))
        }
    }

    fn copy_attribute_value(
        &self,
        attribute: &str,
    ) -> std::result::Result<CFRetained<CFType>, AxErrorCode> {
        let attr = CFString::from_str(attribute);
        let mut value: *const CFType = std::ptr::null();
        let value_ptr: *mut *const CFType = &mut value;

        let result = unsafe {
            self.inner
                .copy_attribute_value(&attr, NonNull::new(value_ptr).unwrap())
        };

        if result == AXError::Success && !value.is_null() {
            let retained =
                unsafe { CFRetained::from_raw(NonNull::new(value as *mut CFType).unwrap()) };
            Ok(retained)
        } else {
            Err(AxErrorCode::from_ax_error(result))
        }
    }

    fn copy_parameterized_attribute_value(
        &self,
        attribute: &str,
        parameter: &CFType,
    ) -> std::result::Result<CFRetained<CFType>, AxErrorCode> {
        let attr = CFString::from_str(attribute);
        let mut value: *const CFType = std::ptr::null();
        let value_ptr: *mut *const CFType = &mut value;

        let result = unsafe {
            self.inner.copy_parameterized_attribute_value(
                &attr,
                parameter,
                NonNull::new(value_ptr).unwrap(),
            )
        };

        if result == AXError::Success && !value.is_null() {
            let retained =
                unsafe { CFRetained::from_raw(NonNull::new(value as *mut CFType).unwrap()) };
            Ok(retained)
        } else {
            Err(AxErrorCode::from_ax_error(result))
        }
    }

    fn array_attribute_values(&self, attribute: &str) -> Vec<AxElement> {
        let attribute = CFString::from_str(attribute);
        let mut count: CFIndex = 0;
        let result = unsafe {
            self.inner
                .attribute_value_count(&attribute, NonNull::new(&mut count).unwrap())
        };
        if result != AXError::Success || count <= 0 {
            return Vec::new();
        }

        let mut values = Vec::new();
        let mut index: CFIndex = 0;
        while index < count {
            let max_values = (count - index).min(256);
            let mut array: *const CFArray = std::ptr::null();
            let result = unsafe {
                self.inner.copy_attribute_values(
                    &attribute,
                    index,
                    max_values,
                    NonNull::new(&mut array).unwrap(),
                )
            };
            if result != AXError::Success || array.is_null() {
                break;
            }

            let array = NonNull::new(array as *mut CFArray as *mut CFArray<AXUIElement>).unwrap();
            let array: CFRetained<CFArray<AXUIElement>> = unsafe { CFRetained::from_raw(array) };
            for i in 0..array.len() {
                if let Some(element) = array.get(i) {
                    values.push(Self::new(element));
                }
            }

            index += max_values;
        }

        values
    }
}

impl AxObserver {
    pub fn new(pid: u32) -> std::result::Result<Self, AxErrorCode> {
        let mut observer_ptr: *mut AXObserver = std::ptr::null_mut();
        let Some(out_observer) = NonNull::new(&mut observer_ptr as *mut *mut AXObserver) else {
            return Err(AxErrorCode::FAILURE);
        };

        let result = unsafe {
            AXObserver::create(
                pid as libc::pid_t,
                Some(ax_notification_callback),
                out_observer,
            )
        };
        if result != AXError::Success {
            return Err(AxErrorCode::from_ax_error(result));
        }

        let Some(observer_ptr) = NonNull::new(observer_ptr) else {
            return Err(AxErrorCode::FAILURE);
        };

        Ok(Self {
            inner: unsafe { CFRetained::from_raw(observer_ptr) },
        })
    }

    pub fn add_notification(
        &self,
        element: &AxElement,
        notification: &str,
        notified: &AtomicBool,
    ) -> AxErrorCode {
        let notification = CFString::from_str(notification);
        AxErrorCode::from_ax_error(unsafe {
            self.inner.add_notification(
                &element.inner,
                &notification,
                notified as *const AtomicBool as *mut c_void,
            )
        })
    }

    pub fn add_notifications(
        &self,
        element: &AxElement,
        notifications: &[&str],
        notified: &AtomicBool,
    ) {
        for notification in notifications {
            let _ = self.add_notification(element, notification, notified);
        }
    }

    pub fn run_loop_source(&self) -> RunLoopSource {
        RunLoopSource {
            inner: unsafe { self.inner.run_loop_source() },
        }
    }
}

impl RunLoop {
    pub fn current() -> Option<Self> {
        CFRunLoop::current().map(|inner| Self { inner })
    }

    pub fn add_default_source(&self, source: &RunLoopSource) {
        self.inner
            .add_source(Some(&source.inner), default_run_loop_mode());
    }

    pub fn remove_default_source(&self, source: &RunLoopSource) {
        self.inner
            .remove_source(Some(&source.inner), default_run_loop_mode());
    }
}

pub fn run_default_loop_slice(seconds: f64, return_after_source_handled: bool) {
    CFRunLoop::run_in_mode(
        default_run_loop_mode(),
        seconds,
        return_after_source_handled,
    );
}

fn ax_result(result: AXError) -> std::result::Result<(), AxErrorCode> {
    if result == AXError::Success {
        Ok(())
    } else {
        Err(AxErrorCode::from_ax_error(result))
    }
}
