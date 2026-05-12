use super::symbols::skylight_event_post_to_pid;
use super::{ModifierFlags, MouseButton, MouseEventKind, Point, WindowId};
use anyhow::{Result, anyhow, bail};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventType, CGMouseButton, CGScrollEventUnit,
};

pub fn current_mouse_location() -> Result<Point> {
    let event =
        CGEvent::new(None).ok_or_else(|| anyhow!("Failed to read current mouse location"))?;
    let point = CGEvent::location(Some(&event));
    Ok(Point::new(point.x, point.y))
}

pub fn post_keyboard_event(
    pid: Option<u32>,
    key_code: u16,
    modifiers: ModifierFlags,
    key_down: bool,
) -> Result<()> {
    let event = CGEvent::new_keyboard_event(None, key_code, key_down)
        .ok_or_else(|| anyhow!("Failed to create keyboard event"))?;
    CGEvent::set_flags(Some(&event), modifier_flags(modifiers));
    post_event(pid, &event)
}

#[allow(clippy::too_many_arguments)]
pub fn post_mouse_event(
    pid: Option<u32>,
    window_id: Option<WindowId>,
    x: f64,
    y: f64,
    kind: MouseEventKind,
    button: MouseButton,
    click_state: i64,
    pressure: f64,
) -> Result<()> {
    let point = objc2_core_foundation::CGPoint { x, y };
    let event_type = mouse_event_type(kind, button);
    let cg_button = cg_mouse_button(button);
    let event = CGEvent::new_mouse_event(None, event_type, point, cg_button)
        .ok_or_else(|| anyhow!("Failed to create mouse event"))?;
    configure_mouse_event(&event, pid, window_id, button, click_state, pressure);
    post_event(pid, &event)
}

pub fn post_scroll_event(pid: Option<u32>, delta_x: f64, delta_y: f64) -> Result<()> {
    let event = CGEvent::new_scroll_wheel_event2(
        None,
        CGScrollEventUnit::Pixel,
        2,
        delta_y.round() as i32,
        delta_x.round() as i32,
        0,
    )
    .ok_or_else(|| anyhow!("Failed to create scroll event"))?;
    post_event(pid, &event)
}

fn modifier_flags(modifiers: ModifierFlags) -> CGEventFlags {
    let mut flags = CGEventFlags::empty();
    if modifiers.shift {
        flags |= CGEventFlags::MaskShift;
    }
    if modifiers.control {
        flags |= CGEventFlags::MaskControl;
    }
    if modifiers.alt {
        flags |= CGEventFlags::MaskAlternate;
    }
    if modifiers.meta {
        flags |= CGEventFlags::MaskCommand;
    }
    flags
}

fn cg_mouse_button(button: MouseButton) -> CGMouseButton {
    match button {
        MouseButton::Left => CGMouseButton::Left,
        MouseButton::Right => CGMouseButton::Right,
        MouseButton::Middle => CGMouseButton::Center,
    }
}

fn mouse_event_type(kind: MouseEventKind, button: MouseButton) -> CGEventType {
    match (kind, button) {
        (MouseEventKind::Move, _) => CGEventType::MouseMoved,
        (MouseEventKind::Down, MouseButton::Left) => CGEventType::LeftMouseDown,
        (MouseEventKind::Up, MouseButton::Left) => CGEventType::LeftMouseUp,
        (MouseEventKind::Down, MouseButton::Right) => CGEventType::RightMouseDown,
        (MouseEventKind::Up, MouseButton::Right) => CGEventType::RightMouseUp,
        (MouseEventKind::Down, MouseButton::Middle) => CGEventType::OtherMouseDown,
        (MouseEventKind::Up, MouseButton::Middle) => CGEventType::OtherMouseUp,
    }
}

fn mouse_button_number(button: MouseButton) -> i64 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Right => 1,
        MouseButton::Middle => 2,
    }
}

fn configure_mouse_event(
    event: &CGEvent,
    pid: Option<u32>,
    window_id: Option<WindowId>,
    button: MouseButton,
    click_state: i64,
    pressure: f64,
) {
    if let Some(pid) = pid {
        set_event_target_pid(event, pid);
        if let Some(window_id) = window_id {
            CGEvent::set_integer_value_field(
                Some(event),
                CGEventField::MouseEventWindowUnderMousePointer,
                window_id.0 as i64,
            );
            CGEvent::set_integer_value_field(
                Some(event),
                CGEventField::MouseEventWindowUnderMousePointerThatCanHandleThisEvent,
                window_id.0 as i64,
            );
        }
    }

    CGEvent::set_integer_value_field(
        Some(event),
        CGEventField::MouseEventButtonNumber,
        mouse_button_number(button),
    );
    CGEvent::set_integer_value_field(Some(event), CGEventField::MouseEventClickState, click_state);
    CGEvent::set_integer_value_field(Some(event), CGEventField::MouseEventSubtype, 0);
    CGEvent::set_double_value_field(Some(event), CGEventField::MouseEventPressure, pressure);
}

fn set_event_target_pid(event: &CGEvent, pid: u32) {
    CGEvent::set_integer_value_field(
        Some(event),
        CGEventField::EventTargetUnixProcessID,
        pid as i64,
    );
}

fn post_event(pid: Option<u32>, event: &CGEvent) -> Result<()> {
    let pid = pid.ok_or_else(|| {
        anyhow!("post_event requires a target pid on macOS (SkyLight has no global path)")
    })?;
    if !post_event_to_pid_via_skylight(pid, event) {
        bail!(
            "SkyLight SLEventPostToPid is unavailable; refusing to fall back to a focus-stealing post"
        );
    }
    Ok(())
}

fn post_event_to_pid_via_skylight(pid: u32, event: &CGEvent) -> bool {
    let Some(post_to_pid) = skylight_event_post_to_pid() else {
        return false;
    };

    set_event_target_pid(event, pid);
    unsafe {
        post_to_pid(pid as libc::pid_t, Some(event));
    }
    true
}
