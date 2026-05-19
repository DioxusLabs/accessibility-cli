use super::*;

/// Convert a keyboard-types Code to a Windows virtual key code.
pub(super) fn code_to_vk(key: Code) -> VIRTUAL_KEY {
    match key {
        Code::KeyA => VIRTUAL_KEY(0x41),
        Code::KeyB => VIRTUAL_KEY(0x42),
        Code::KeyC => VIRTUAL_KEY(0x43),
        Code::KeyD => VIRTUAL_KEY(0x44),
        Code::KeyE => VIRTUAL_KEY(0x45),
        Code::KeyF => VIRTUAL_KEY(0x46),
        Code::KeyG => VIRTUAL_KEY(0x47),
        Code::KeyH => VIRTUAL_KEY(0x48),
        Code::KeyI => VIRTUAL_KEY(0x49),
        Code::KeyJ => VIRTUAL_KEY(0x4A),
        Code::KeyK => VIRTUAL_KEY(0x4B),
        Code::KeyL => VIRTUAL_KEY(0x4C),
        Code::KeyM => VIRTUAL_KEY(0x4D),
        Code::KeyN => VIRTUAL_KEY(0x4E),
        Code::KeyO => VIRTUAL_KEY(0x4F),
        Code::KeyP => VIRTUAL_KEY(0x50),
        Code::KeyQ => VIRTUAL_KEY(0x51),
        Code::KeyR => VIRTUAL_KEY(0x52),
        Code::KeyS => VIRTUAL_KEY(0x53),
        Code::KeyT => VIRTUAL_KEY(0x54),
        Code::KeyU => VIRTUAL_KEY(0x55),
        Code::KeyV => VIRTUAL_KEY(0x56),
        Code::KeyW => VIRTUAL_KEY(0x57),
        Code::KeyX => VIRTUAL_KEY(0x58),
        Code::KeyY => VIRTUAL_KEY(0x59),
        Code::KeyZ => VIRTUAL_KEY(0x5A),
        Code::Digit0 => VIRTUAL_KEY(0x30),
        Code::Digit1 => VIRTUAL_KEY(0x31),
        Code::Digit2 => VIRTUAL_KEY(0x32),
        Code::Digit3 => VIRTUAL_KEY(0x33),
        Code::Digit4 => VIRTUAL_KEY(0x34),
        Code::Digit5 => VIRTUAL_KEY(0x35),
        Code::Digit6 => VIRTUAL_KEY(0x36),
        Code::Digit7 => VIRTUAL_KEY(0x37),
        Code::Digit8 => VIRTUAL_KEY(0x38),
        Code::Digit9 => VIRTUAL_KEY(0x39),
        Code::F1 => VK_F1,
        Code::F2 => VK_F2,
        Code::F3 => VK_F3,
        Code::F4 => VK_F4,
        Code::F5 => VK_F5,
        Code::F6 => VK_F6,
        Code::F7 => VK_F7,
        Code::F8 => VK_F8,
        Code::F9 => VK_F9,
        Code::F10 => VK_F10,
        Code::F11 => VK_F11,
        Code::F12 => VK_F12,
        Code::F13 => VK_F13,
        Code::F14 => VK_F14,
        Code::F15 => VK_F15,
        Code::F16 => VK_F16,
        Code::F17 => VK_F17,
        Code::F18 => VK_F18,
        Code::F19 => VK_F19,
        Code::F20 => VK_F20,
        Code::Enter => VK_RETURN,
        Code::Tab => VK_TAB,
        Code::Space => VK_SPACE,
        Code::Backspace => VK_BACK,
        Code::Escape => VK_ESCAPE,
        Code::Delete => VK_DELETE,
        Code::Insert => VK_INSERT,
        Code::Home => VK_HOME,
        Code::End => VK_END,
        Code::PageUp => VK_PRIOR,
        Code::PageDown => VK_NEXT,
        Code::ArrowUp => VK_UP,
        Code::ArrowDown => VK_DOWN,
        Code::ArrowLeft => VK_LEFT,
        Code::ArrowRight => VK_RIGHT,
        Code::ShiftLeft | Code::ShiftRight => VK_SHIFT,
        Code::ControlLeft | Code::ControlRight => VK_CONTROL,
        Code::AltLeft | Code::AltRight => VK_MENU,
        Code::MetaLeft | Code::MetaRight => VK_LWIN,
        Code::Minus => VK_OEM_MINUS,
        Code::Equal => VK_OEM_PLUS,
        Code::BracketLeft => VK_OEM_4,
        Code::BracketRight => VK_OEM_6,
        Code::Backslash => VK_OEM_5,
        Code::Semicolon => VK_OEM_1,
        Code::Quote => VK_OEM_7,
        Code::Backquote => VK_OEM_3,
        Code::Comma => VK_OEM_COMMA,
        Code::Period => VK_OEM_PERIOD,
        Code::Slash => VK_OEM_2,
        Code::Numpad0 => VK_NUMPAD0,
        Code::Numpad1 => VK_NUMPAD1,
        Code::Numpad2 => VK_NUMPAD2,
        Code::Numpad3 => VK_NUMPAD3,
        Code::Numpad4 => VK_NUMPAD4,
        Code::Numpad5 => VK_NUMPAD5,
        Code::Numpad6 => VK_NUMPAD6,
        Code::Numpad7 => VK_NUMPAD7,
        Code::Numpad8 => VK_NUMPAD8,
        Code::Numpad9 => VK_NUMPAD9,
        Code::NumpadDecimal => VIRTUAL_KEY(0x6E),
        Code::NumpadMultiply => VIRTUAL_KEY(0x6A),
        Code::NumpadAdd => VIRTUAL_KEY(0x6B),
        Code::NumpadSubtract => VIRTUAL_KEY(0x6D),
        Code::NumpadDivide => VIRTUAL_KEY(0x6F),
        Code::NumpadEnter => VK_RETURN, // Same as regular return
        Code::CapsLock => VK_CAPITAL,
        Code::NumLock => VK_NUMLOCK,
        Code::ScrollLock => VK_SCROLL,
        Code::AudioVolumeUp => VK_VOLUME_UP,
        Code::AudioVolumeDown => VK_VOLUME_DOWN,
        Code::AudioVolumeMute => VK_VOLUME_MUTE,
        Code::MediaPlayPause => VK_MEDIA_PLAY_PAUSE,
        Code::MediaStop => VK_MEDIA_STOP,
        Code::MediaTrackNext => VK_MEDIA_NEXT_TRACK,
        Code::MediaTrackPrevious => VK_MEDIA_PREV_TRACK,
        Code::PrintScreen => VK_SNAPSHOT,
        _ => VK_CANCEL, // Unsupported key, return cancel
    }
}

/// Check if a virtual key is an extended key.
/// Extended keys include: arrows, Insert, Delete, Home, End, Page Up, Page Down,
/// Num Lock, Break, Print Screen, and right-hand Alt/Ctrl.
fn is_extended_key(vk: VIRTUAL_KEY) -> bool {
    matches!(
        vk,
        VK_UP | VK_DOWN | VK_LEFT | VK_RIGHT |
        VK_INSERT | VK_DELETE | VK_HOME | VK_END |
        VK_PRIOR | VK_NEXT |  // Page Up / Page Down
        VK_NUMLOCK | VK_CANCEL | VK_SNAPSHOT |  // Num Lock, Break, Print Screen
        VK_DIVIDE |  // Numpad divide
        VK_RCONTROL | VK_RMENU // Right Ctrl, Right Alt
    )
}

/// Send a keyboard event.
pub(super) fn send_key_event(vk: VIRTUAL_KEY, key_up: bool) -> Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{MAP_VIRTUAL_KEY_TYPE, MapVirtualKeyW};

    let mut flags = KEYBD_EVENT_FLAGS(0);
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    if is_extended_key(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }

    // MAPVK_VK_TO_VSC = 0
    let scan_code = unsafe { MapVirtualKeyW(vk.0 as u32, MAP_VIRTUAL_KEY_TYPE(0)) as u16 };

    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan_code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    let inserted = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    if inserted != 1 {
        bail!("SendInput failed to insert keyboard event");
    }
    Ok(())
}

/// Get the PID of the foreground window.
pub fn get_foreground_pid() -> Option<u32> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 { None } else { Some(pid) }
}
