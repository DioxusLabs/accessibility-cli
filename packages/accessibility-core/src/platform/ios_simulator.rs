//! iOS Simulator accessibility and HID support.
//!
//! The raw Objective-C, CoreFoundation, private-framework loading, and Indigo HID
//! message construction live in `accessibility-ios-sys`. This module keeps the
//! public core API platform-agnostic by converting sys snapshots into core
//! `Element` values and core-owned `ElementKey`s.

use std::collections::HashMap;
use std::future;
use std::sync::Arc;

use accessibility_ios_sys as sys;
use accesskit::{Action, Role};
use anyhow::{Result, anyhow};
use slotmap::SecondaryMap;

use crate::accessibility::{
    AccessibilityReader, Element, ElementCache, ElementKey, ElementTree, Point, Rect, Screenshot,
    Size, Target, TreeFilter,
};
use crate::video::{
    EncodedFrame, FrameKind, FrameSink, NalFormat, Recording, RecordingConfig, ScreenGeometry,
    Tuning, VideoCapture, VideoConfig,
};

pub use sys::{ButtonDirection, HardwareButton};

/// Load all required private frameworks.
pub fn load_frameworks() -> Result<()> {
    sys::load_frameworks()
}

/// iOS Simulator accessibility reader.
///
/// This is a safe core wrapper around `accessibility-ios-sys`; it does not expose
/// Objective-C, CoreFoundation, or libc handles outside the sys crate.
pub struct IOSSimulatorAccessibility {
    inner: sys::IOSSimulatorAccessibility,
    cache: ElementCache,
    sys_ids: SecondaryMap<ElementKey, sys::ElementKey>,
    core_ids: HashMap<u64, ElementKey>,
}

impl IOSSimulatorAccessibility {
    /// Create a new iOS Simulator accessibility reader.
    ///
    /// If `udid` is None, uses the first booted simulator found.
    pub fn new(udid: Option<&str>) -> Result<Self> {
        Ok(Self {
            inner: sys::IOSSimulatorAccessibility::new(udid)?,
            cache: ElementCache::new(),
            sys_ids: SecondaryMap::new(),
            core_ids: HashMap::new(),
        })
    }

    /// Get the device UDID.
    pub fn device_udid(&self) -> &str {
        self.inner.device_udid()
    }

    /// Get the accessibility tree from the frontmost app in the simulator.
    pub fn get_tree(&mut self, filter: &TreeFilter) -> Result<ElementTree> {
        self.clear_local_cache();

        let sys_tree = self.inner.get_tree(&to_sys_filter(filter))?;
        let root = self.map_element(&sys_tree.root);
        let element_count = count_elements(&root);

        Ok(ElementTree {
            version: self.cache.version(),
            pid: sys_tree.pid,
            app_name: sys_tree.app_name,
            root,
            element_count,
        })
    }

    /// Get a cached core element by ID.
    pub fn get_element(&self, id: ElementKey) -> Option<&Element> {
        self.cache.get(id)
    }

    /// Clear both the sys snapshot and the core ID mapping.
    pub fn clear_cache(&mut self) {
        self.inner.clear_cache();
        self.clear_local_cache();
    }

    /// Get the current core snapshot version.
    pub fn snapshot_version(&self) -> u64 {
        self.cache.version()
    }

    /// Perform an action on an element by core ID.
    pub fn perform_action(&mut self, id: ElementKey, action: Action) -> Result<()> {
        let sys_id = self.sys_id(id)?;
        self.inner.perform_action(sys_id, action)
    }

    /// Perform a press action on an element by core ID.
    pub fn press(&mut self, id: ElementKey) -> Result<()> {
        let sys_id = self.sys_id(id)?;
        self.inner.press(sys_id)
    }

    /// Set text value on a text field element.
    pub fn set_value(&mut self, id: ElementKey, value: &str) -> Result<()> {
        let sys_id = self.sys_id(id)?;
        self.inner.set_value(sys_id, value)
    }

    /// Tap at screen coordinates using the accessibility API.
    pub fn tap(&mut self, x: f64, y: f64) -> Result<()> {
        self.inner.tap(x, y)
    }

    /// Get element at screen coordinates.
    pub fn element_at_point(&mut self, x: f64, y: f64) -> Result<Option<Element>> {
        self.inner
            .element_at_point(x, y)?
            .map(|element| Ok(self.map_element(&element)))
            .transpose()
    }

    /// Get the screen size in points.
    pub fn screen_size(&mut self) -> Result<(f64, f64)> {
        self.inner.screen_size()
    }

    /// Tap at screen coordinates using HID injection.
    pub fn hid_tap(&mut self, x: f64, y: f64) -> Result<()> {
        self.inner.hid_tap(x, y)
    }

    /// Perform a swipe gesture using HID injection.
    pub fn hid_swipe(
        &mut self,
        start: (f64, f64),
        end: (f64, f64),
        duration_ms: u64,
    ) -> Result<()> {
        self.inner.hid_swipe(start, end, duration_ms)
    }

    /// Press a hardware button using HID injection.
    pub fn hid_button(&mut self, button: HardwareButton, hold_ms: u64) -> Result<()> {
        self.inner.hid_button(button, hold_ms)
    }

    /// Send a keyboard key press using HID injection.
    pub fn hid_key(&mut self, key_code: u32) -> Result<()> {
        self.inner.hid_key(key_code)
    }

    /// Capture a screenshot of the entire simulator screen.
    pub fn capture_screen(&self) -> Result<Screenshot> {
        self.inner.capture_screen().map(from_sys_screenshot)
    }

    /// Get the screen bounds for the simulator.
    pub fn get_screen_bounds(&self) -> Result<Rect> {
        self.inner
            .get_screen_bounds()
            .map(|rect| from_sys_rect(&rect))
    }

    /// Capture a screenshot of a specific element.
    pub fn capture_element(&mut self, id: ElementKey) -> Result<Screenshot> {
        let sys_id = self.sys_id(id)?;
        self.inner.capture_element(sys_id).map(from_sys_screenshot)
    }

    /// Test helper kept on the public wrapper for existing unit tests.
    pub fn map_role(role: &str) -> Role {
        sys::IOSSimulatorAccessibility::map_role(role)
    }

    /// Test helper kept on the public wrapper for existing unit tests.
    pub fn is_interactive(role: &Role, actions: &[String]) -> bool {
        sys::IOSSimulatorAccessibility::is_interactive(role, actions)
    }

    fn clear_local_cache(&mut self) {
        self.cache.clear();
        self.sys_ids.clear();
        self.core_ids.clear();
    }

    fn sys_id(&self, id: ElementKey) -> Result<sys::ElementKey> {
        self.sys_ids
            .get(id)
            .copied()
            .ok_or_else(|| anyhow!("Element {} not found in cache. Call get_tree() first.", id))
    }

    fn map_element(&mut self, sys_element: &sys::Element) -> Element {
        if let Some(existing) = self.core_ids.get(&sys_element.id.to_ffi()).copied()
            && let Some(element) = self.cache.get(existing)
        {
            return element.clone();
        }

        let children = sys_element
            .children
            .iter()
            .map(|child| self.map_element(child))
            .collect();
        let sys_id = sys_element.id;

        let (id, element) = self.cache.store_with_clone(|id| Element {
            id,
            role: sys_element.role,
            title: sys_element.title.clone(),
            description: sys_element.description.clone(),
            value: sys_element.value.clone(),
            url: sys_element.url.clone(),
            help: sys_element.help.clone(),
            role_description: sys_element.role_description.clone(),
            identifier: sys_element.identifier.clone(),
            bounds: sys_element.bounds.as_ref().map(from_sys_rect),
            enabled: sys_element.enabled,
            focused: sys_element.focused,
            actions: sys_element.actions.clone(),
            children,
        });

        self.sys_ids.insert(id, sys_id);
        self.core_ids.insert(sys_id.to_ffi(), id);
        element
    }
}

impl AccessibilityReader for IOSSimulatorAccessibility {
    fn get_tree(
        &mut self,
        _target: &Target,
        filter: &TreeFilter,
    ) -> impl std::future::Future<Output = Result<ElementTree>> {
        future::ready(IOSSimulatorAccessibility::get_tree(self, filter))
    }

    fn get_element(&self, id: ElementKey) -> Option<&Element> {
        IOSSimulatorAccessibility::get_element(self, id)
    }

    fn perform_action(
        &mut self,
        id: ElementKey,
        action: Action,
    ) -> impl std::future::Future<Output = Result<()>> {
        future::ready(IOSSimulatorAccessibility::perform_action(self, id, action))
    }

    fn set_value(
        &mut self,
        id: ElementKey,
        value: &str,
    ) -> impl std::future::Future<Output = Result<()>> {
        future::ready(IOSSimulatorAccessibility::set_value(self, id, value))
    }

    fn hit_test(
        &mut self,
        x: f64,
        y: f64,
    ) -> impl std::future::Future<Output = Result<Option<ElementKey>>> {
        let result = match self.element_at_point(x, y) {
            Ok(Some(elem)) => Ok(Some(elem.id)),
            Ok(None) => Ok(None),
            Err(error) => Err(error),
        };
        future::ready(result)
    }

    fn clear_cache(&mut self) {
        IOSSimulatorAccessibility::clear_cache(self)
    }

    fn snapshot_version(&self) -> u64 {
        IOSSimulatorAccessibility::snapshot_version(self)
    }

    fn capture_screen(&self, _target: &Target) -> Result<Screenshot> {
        IOSSimulatorAccessibility::capture_screen(self)
    }

    fn get_screen_bounds(
        &self,
        _target: &Target,
    ) -> impl std::future::Future<Output = Result<Rect>> {
        future::ready(IOSSimulatorAccessibility::get_screen_bounds(self))
    }

    fn platform_name(&self) -> &'static str {
        "iOS"
    }

    fn supports_hit_test(&self) -> bool {
        true
    }

    fn start_video_capture(
        &self,
        config: &VideoConfig,
        sink: FrameSink,
    ) -> Result<Box<dyn VideoCapture>> {
        let session = SimulatorVideoCapture::start(self.device_udid(), config, sink)?;
        Ok(Box::new(session))
    }

    fn supports_video_capture(&self) -> bool {
        true
    }
}

/// Live framebuffer capture for the simulator, wrapping `accessibility-ios-sys`.
pub struct SimulatorVideoCapture {
    inner: sys::SimVideoStream,
}

impl SimulatorVideoCapture {
    /// Start capturing the given device.
    ///
    /// The returned session owns the SimulatorKit registration; dropping it
    /// tears the capture pipeline down.
    pub fn start(udid: &str, config: &VideoConfig, sink: FrameSink) -> Result<Self> {
        let sys_config = sys::EncoderConfig {
            fps: config.fps,
            tuning: match config.tuning {
                Tuning::Interactive { bitrate } => sys::Tuning::Interactive { bitrate },
                Tuning::Recording { quality } => sys::Tuning::Recording { quality },
            },
            max_dimension: config.max_dimension,
            keyframe_interval_secs: config.keyframe_interval_secs,
            nal_format: match config.nal_format {
                NalFormat::AnnexB => sys::NalFormat::AnnexB,
                NalFormat::Avcc => sys::NalFormat::Avcc,
            },
        };

        let sys_sink: sys::ChunkSink = Arc::new(move |chunk: sys::EncodedChunk| {
            sink(EncodedFrame {
                data: chunk.data,
                kind: match chunk.kind {
                    sys::ChunkKind::ParameterSet => FrameKind::ParameterSet,
                    sys::ChunkKind::Keyframe => FrameKind::Keyframe,
                    sys::ChunkKind::Delta => FrameKind::Delta,
                },
            });
        });

        Ok(Self {
            inner: sys::SimVideoStream::start(Some(udid), sys_config, sys_sink)?,
        })
    }
}

impl VideoCapture for SimulatorVideoCapture {
    fn geometry(&self) -> ScreenGeometry {
        let geometry = self.inner.geometry();
        ScreenGeometry {
            width: geometry.width,
            height: geometry.height,
        }
    }

    fn encoded_geometry(&self) -> ScreenGeometry {
        let geometry = self.inner.encoded_geometry();
        ScreenGeometry {
            width: geometry.width,
            height: geometry.height,
        }
    }

    fn request_keyframe(&self) {
        self.inner.request_keyframe();
    }

    fn start_recording(&self, path: &std::path::Path, config: &RecordingConfig) -> Result<()> {
        self.inner.start_recording(
            path,
            sys::RecordingConfig {
                quality: config.quality,
                max_dimension: config.max_dimension,
                keyframe_interval_secs: config.keyframe_interval_secs,
            },
        )
    }

    fn stop_recording(&self) -> Result<Recording> {
        let recording = self.inner.stop_recording()?;
        Ok(Recording {
            path: recording.path,
            frames: recording.frames,
            duration_secs: recording.duration.as_secs_f64(),
            width: recording.width,
            height: recording.height,
        })
    }

    fn recording_frames(&self) -> Option<u64> {
        self.inner.recording_frames()
    }

    fn stop(&mut self) {
        self.inner.stop();
    }
}

use super::macos::IOSAdapter;

impl IOSAdapter for IOSSimulatorAccessibility {
    fn hid_tap(&mut self, x: f64, y: f64) -> Result<()> {
        IOSSimulatorAccessibility::hid_tap(self, x, y)
    }

    fn hid_swipe(&mut self, start: (f64, f64), end: (f64, f64), duration_ms: u64) -> Result<()> {
        IOSSimulatorAccessibility::hid_swipe(self, start, end, duration_ms)
    }

    fn hid_button(&mut self, button: HardwareButton, hold_ms: u64) -> Result<()> {
        IOSSimulatorAccessibility::hid_button(self, button, hold_ms)
    }

    fn tap(&mut self, x: f64, y: f64) -> Result<()> {
        IOSSimulatorAccessibility::tap(self, x, y)
    }

    fn press(&mut self, id: ElementKey) -> Result<()> {
        IOSSimulatorAccessibility::press(self, id)
    }

    fn device_udid(&self) -> &str {
        IOSSimulatorAccessibility::device_udid(self)
    }
}

fn to_sys_filter(filter: &TreeFilter) -> sys::TreeFilter {
    sys::TreeFilter {
        max_depth: filter.max_depth,
        max_elements: filter.max_elements,
        interactive_only: filter.interactive_only,
        visible_only: filter.visible_only,
        within_bounds: filter.within_bounds.as_ref().map(to_sys_rect),
        roles: filter.roles.clone(),
    }
}

fn to_sys_rect(rect: &Rect) -> sys::Rect {
    sys::Rect::new(
        sys::Point::new(rect.origin.x, rect.origin.y),
        sys::Size::new(rect.size.width, rect.size.height),
    )
}

fn from_sys_rect(rect: &sys::Rect) -> Rect {
    Rect::new(
        Point::new(rect.origin.x, rect.origin.y),
        Size::new(rect.size.width, rect.size.height),
    )
}

fn from_sys_screenshot(screenshot: sys::Screenshot) -> Screenshot {
    Screenshot {
        data: screenshot.data,
        width: screenshot.width,
        height: screenshot.height,
    }
}

fn count_elements(element: &Element) -> usize {
    1 + element.children.iter().map(count_elements).sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_mapping() {
        assert_eq!(
            IOSSimulatorAccessibility::map_role("AXButton"),
            accesskit::Role::Button
        );
        assert_eq!(
            IOSSimulatorAccessibility::map_role("AXTextField"),
            accesskit::Role::TextInput
        );
        assert_eq!(
            IOSSimulatorAccessibility::map_role("Button"),
            accesskit::Role::Button
        );
        assert_eq!(
            IOSSimulatorAccessibility::map_role("Unknown"),
            accesskit::Role::Unknown
        );
    }

    #[test]
    fn test_is_interactive() {
        assert!(IOSSimulatorAccessibility::is_interactive(
            &accesskit::Role::Button,
            &[]
        ));
        assert!(IOSSimulatorAccessibility::is_interactive(
            &accesskit::Role::Unknown,
            &["AXPress".to_string()]
        ));
        assert!(!IOSSimulatorAccessibility::is_interactive(
            &accesskit::Role::Label,
            &[]
        ));
    }
}
