//! Input types for raw keystroke and mouse injection.
//!
//! Re-exports keyboard types from the `keyboard-types` crate and provides
//! additional helpers for parsing and conversion.

use std::fmt;
use std::str::FromStr;

// Re-export keyboard-types for use throughout the crate
pub use keyboard_types::{Code, Key, Modifiers};

/// Parse a key name string into a Code.
///
/// Supports various formats:
/// - Single letters: "a", "A" (both map to KeyA)
/// - Numbers: "0", "1", etc. (map to Digit0, Digit1, etc.)
/// - Named keys: "enter", "return", "tab", "escape", "space", etc.
/// - Function keys: "f1", "F1", etc.
/// - Modifiers: "shift", "ctrl", "control", "alt", "option", "meta", "cmd", "command"
pub fn parse_key_code(name: &str) -> Option<Code> {
    let lower = name.to_lowercase();

    // Single character
    if name.len() == 1 {
        let c = name.chars().next().unwrap();
        return match c.to_ascii_lowercase() {
            'a' => Some(Code::KeyA),
            'b' => Some(Code::KeyB),
            'c' => Some(Code::KeyC),
            'd' => Some(Code::KeyD),
            'e' => Some(Code::KeyE),
            'f' => Some(Code::KeyF),
            'g' => Some(Code::KeyG),
            'h' => Some(Code::KeyH),
            'i' => Some(Code::KeyI),
            'j' => Some(Code::KeyJ),
            'k' => Some(Code::KeyK),
            'l' => Some(Code::KeyL),
            'm' => Some(Code::KeyM),
            'n' => Some(Code::KeyN),
            'o' => Some(Code::KeyO),
            'p' => Some(Code::KeyP),
            'q' => Some(Code::KeyQ),
            'r' => Some(Code::KeyR),
            's' => Some(Code::KeyS),
            't' => Some(Code::KeyT),
            'u' => Some(Code::KeyU),
            'v' => Some(Code::KeyV),
            'w' => Some(Code::KeyW),
            'x' => Some(Code::KeyX),
            'y' => Some(Code::KeyY),
            'z' => Some(Code::KeyZ),
            '0' => Some(Code::Digit0),
            '1' => Some(Code::Digit1),
            '2' => Some(Code::Digit2),
            '3' => Some(Code::Digit3),
            '4' => Some(Code::Digit4),
            '5' => Some(Code::Digit5),
            '6' => Some(Code::Digit6),
            '7' => Some(Code::Digit7),
            '8' => Some(Code::Digit8),
            '9' => Some(Code::Digit9),
            '-' => Some(Code::Minus),
            '=' => Some(Code::Equal),
            '[' => Some(Code::BracketLeft),
            ']' => Some(Code::BracketRight),
            '\\' => Some(Code::Backslash),
            ';' => Some(Code::Semicolon),
            '\'' => Some(Code::Quote),
            '`' => Some(Code::Backquote),
            ',' => Some(Code::Comma),
            '.' => Some(Code::Period),
            '/' => Some(Code::Slash),
            ' ' => Some(Code::Space),
            _ => None,
        };
    }

    // Try the keyboard-types FromStr implementation first
    if let Ok(code) = Code::from_str(&lower) {
        return Some(code);
    }

    // Named keys (custom aliases)
    match lower.as_str() {
        // Enter/Return
        "enter" | "return" => Some(Code::Enter),

        // Tab
        "tab" => Some(Code::Tab),

        // Space
        "space" => Some(Code::Space),

        // Backspace/Delete
        "backspace" | "back" => Some(Code::Backspace),
        "delete" | "del" => Some(Code::Delete),

        // Escape
        "escape" | "esc" => Some(Code::Escape),

        // Insert
        "insert" | "ins" => Some(Code::Insert),

        // Navigation
        "home" => Some(Code::Home),
        "end" => Some(Code::End),
        "pageup" | "page_up" | "pgup" => Some(Code::PageUp),
        "pagedown" | "page_down" | "pgdn" => Some(Code::PageDown),

        // Arrows
        "up" | "arrowup" | "uparrow" => Some(Code::ArrowUp),
        "down" | "arrowdown" | "downarrow" => Some(Code::ArrowDown),
        "left" | "arrowleft" | "leftarrow" => Some(Code::ArrowLeft),
        "right" | "arrowright" | "rightarrow" => Some(Code::ArrowRight),

        // Modifiers
        "shift" | "shiftleft" => Some(Code::ShiftLeft),
        "shiftright" => Some(Code::ShiftRight),
        "ctrl" | "control" | "controlleft" => Some(Code::ControlLeft),
        "controlright" => Some(Code::ControlRight),
        "alt" | "option" | "altleft" => Some(Code::AltLeft),
        "altright" => Some(Code::AltRight),
        "meta" | "cmd" | "command" | "win" | "super" | "metaleft" => Some(Code::MetaLeft),
        "metaright" => Some(Code::MetaRight),

        // Function keys
        "f1" => Some(Code::F1),
        "f2" => Some(Code::F2),
        "f3" => Some(Code::F3),
        "f4" => Some(Code::F4),
        "f5" => Some(Code::F5),
        "f6" => Some(Code::F6),
        "f7" => Some(Code::F7),
        "f8" => Some(Code::F8),
        "f9" => Some(Code::F9),
        "f10" => Some(Code::F10),
        "f11" => Some(Code::F11),
        "f12" => Some(Code::F12),
        "f13" => Some(Code::F13),
        "f14" => Some(Code::F14),
        "f15" => Some(Code::F15),
        "f16" => Some(Code::F16),
        "f17" => Some(Code::F17),
        "f18" => Some(Code::F18),
        "f19" => Some(Code::F19),
        "f20" => Some(Code::F20),

        // Lock keys
        "capslock" | "caps_lock" | "caps" => Some(Code::CapsLock),
        "numlock" | "num_lock" => Some(Code::NumLock),
        "scrolllock" | "scroll_lock" => Some(Code::ScrollLock),

        // Punctuation by name
        "minus" => Some(Code::Minus),
        "equal" | "equals" => Some(Code::Equal),
        "leftbracket" | "lbracket" | "bracketleft" => Some(Code::BracketLeft),
        "rightbracket" | "rbracket" | "bracketright" => Some(Code::BracketRight),
        "backslash" => Some(Code::Backslash),
        "semicolon" => Some(Code::Semicolon),
        "quote" | "apostrophe" => Some(Code::Quote),
        "grave" | "backtick" | "tilde" | "backquote" => Some(Code::Backquote),
        "comma" => Some(Code::Comma),
        "period" | "dot" => Some(Code::Period),
        "slash" | "forwardslash" => Some(Code::Slash),

        // Media
        "volumeup" | "volume_up" => Some(Code::AudioVolumeUp),
        "volumedown" | "volume_down" => Some(Code::AudioVolumeDown),
        "volumemute" | "volume_mute" | "mute" => Some(Code::AudioVolumeMute),
        "playpause" | "play_pause" | "play" => Some(Code::MediaPlayPause),
        "mediastop" | "stop" => Some(Code::MediaStop),
        "medianext" | "next" | "nexttrack" => Some(Code::MediaTrackNext),
        "mediaprevious" | "previous" | "prevtrack" => Some(Code::MediaTrackPrevious),

        // Print screen
        "printscreen" | "print_screen" | "prtsc" | "screenshot" => Some(Code::PrintScreen),

        // Numpad
        "numpad0" => Some(Code::Numpad0),
        "numpad1" => Some(Code::Numpad1),
        "numpad2" => Some(Code::Numpad2),
        "numpad3" => Some(Code::Numpad3),
        "numpad4" => Some(Code::Numpad4),
        "numpad5" => Some(Code::Numpad5),
        "numpad6" => Some(Code::Numpad6),
        "numpad7" => Some(Code::Numpad7),
        "numpad8" => Some(Code::Numpad8),
        "numpad9" => Some(Code::Numpad9),
        "numpaddecimal" => Some(Code::NumpadDecimal),
        "numpadmultiply" => Some(Code::NumpadMultiply),
        "numpadadd" => Some(Code::NumpadAdd),
        "numpadsubtract" => Some(Code::NumpadSubtract),
        "numpaddivide" => Some(Code::NumpadDivide),
        "numpadenter" => Some(Code::NumpadEnter),

        _ => None,
    }
}

/// Get the Code for a character.
///
/// Returns the key code and whether shift is needed.
pub fn code_from_char(c: char) -> Option<(Code, bool)> {
    match c {
        // Lowercase letters
        'a'..='z' => {
            let codes = [
                Code::KeyA,
                Code::KeyB,
                Code::KeyC,
                Code::KeyD,
                Code::KeyE,
                Code::KeyF,
                Code::KeyG,
                Code::KeyH,
                Code::KeyI,
                Code::KeyJ,
                Code::KeyK,
                Code::KeyL,
                Code::KeyM,
                Code::KeyN,
                Code::KeyO,
                Code::KeyP,
                Code::KeyQ,
                Code::KeyR,
                Code::KeyS,
                Code::KeyT,
                Code::KeyU,
                Code::KeyV,
                Code::KeyW,
                Code::KeyX,
                Code::KeyY,
                Code::KeyZ,
            ];
            let idx = (c as u8 - b'a') as usize;
            Some((codes[idx], false))
        }
        // Uppercase letters need shift
        'A'..='Z' => {
            let codes = [
                Code::KeyA,
                Code::KeyB,
                Code::KeyC,
                Code::KeyD,
                Code::KeyE,
                Code::KeyF,
                Code::KeyG,
                Code::KeyH,
                Code::KeyI,
                Code::KeyJ,
                Code::KeyK,
                Code::KeyL,
                Code::KeyM,
                Code::KeyN,
                Code::KeyO,
                Code::KeyP,
                Code::KeyQ,
                Code::KeyR,
                Code::KeyS,
                Code::KeyT,
                Code::KeyU,
                Code::KeyV,
                Code::KeyW,
                Code::KeyX,
                Code::KeyY,
                Code::KeyZ,
            ];
            let idx = (c as u8 - b'A') as usize;
            Some((codes[idx], true))
        }
        // Numbers
        '0' => Some((Code::Digit0, false)),
        '1' => Some((Code::Digit1, false)),
        '2' => Some((Code::Digit2, false)),
        '3' => Some((Code::Digit3, false)),
        '4' => Some((Code::Digit4, false)),
        '5' => Some((Code::Digit5, false)),
        '6' => Some((Code::Digit6, false)),
        '7' => Some((Code::Digit7, false)),
        '8' => Some((Code::Digit8, false)),
        '9' => Some((Code::Digit9, false)),
        // Shifted number row symbols (US layout)
        '!' => Some((Code::Digit1, true)),
        '@' => Some((Code::Digit2, true)),
        '#' => Some((Code::Digit3, true)),
        '$' => Some((Code::Digit4, true)),
        '%' => Some((Code::Digit5, true)),
        '^' => Some((Code::Digit6, true)),
        '&' => Some((Code::Digit7, true)),
        '*' => Some((Code::Digit8, true)),
        '(' => Some((Code::Digit9, true)),
        ')' => Some((Code::Digit0, true)),
        // Punctuation
        '-' => Some((Code::Minus, false)),
        '_' => Some((Code::Minus, true)),
        '=' => Some((Code::Equal, false)),
        '+' => Some((Code::Equal, true)),
        '[' => Some((Code::BracketLeft, false)),
        '{' => Some((Code::BracketLeft, true)),
        ']' => Some((Code::BracketRight, false)),
        '}' => Some((Code::BracketRight, true)),
        '\\' => Some((Code::Backslash, false)),
        '|' => Some((Code::Backslash, true)),
        ';' => Some((Code::Semicolon, false)),
        ':' => Some((Code::Semicolon, true)),
        '\'' => Some((Code::Quote, false)),
        '"' => Some((Code::Quote, true)),
        '`' => Some((Code::Backquote, false)),
        '~' => Some((Code::Backquote, true)),
        ',' => Some((Code::Comma, false)),
        '<' => Some((Code::Comma, true)),
        '.' => Some((Code::Period, false)),
        '>' => Some((Code::Period, true)),
        '/' => Some((Code::Slash, false)),
        '?' => Some((Code::Slash, true)),
        ' ' => Some((Code::Space, false)),
        '\n' => Some((Code::Enter, false)),
        '\t' => Some((Code::Tab, false)),
        _ => None,
    }
}

/// Parse modifier string like "ctrl+shift" or "cmd".
///
/// Supports: shift, ctrl, control, alt, option, meta, cmd, command, win, super
pub fn parse_modifiers(s: &str) -> Modifiers {
    let mut mods = Modifiers::empty();
    for part in s.to_lowercase().split('+') {
        match part.trim() {
            "shift" => mods |= Modifiers::SHIFT,
            "ctrl" | "control" => mods |= Modifiers::CONTROL,
            "alt" | "option" => mods |= Modifiers::ALT,
            "meta" | "cmd" | "command" | "win" | "super" => mods |= Modifiers::META,
            _ => {}
        }
    }
    mods
}

/// Mouse button for click operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseButton {
    #[default]
    Left,
    Right,
    Middle,
}

impl MouseButton {
    /// Parse a button name.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "left" | "l" | "1" => Some(MouseButton::Left),
            "right" | "r" | "2" => Some(MouseButton::Right),
            "middle" | "m" | "3" => Some(MouseButton::Middle),
            _ => None,
        }
    }
}

impl fmt::Display for MouseButton {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MouseButton::Left => write!(f, "Left"),
            MouseButton::Right => write!(f, "Right"),
            MouseButton::Middle => write!(f, "Middle"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_key_code() {
        assert_eq!(parse_key_code("a"), Some(Code::KeyA));
        assert_eq!(parse_key_code("A"), Some(Code::KeyA));
        assert_eq!(parse_key_code("enter"), Some(Code::Enter));
        assert_eq!(parse_key_code("Return"), Some(Code::Enter));
        assert_eq!(parse_key_code("f1"), Some(Code::F1));
        assert_eq!(parse_key_code("F12"), Some(Code::F12));
        assert_eq!(parse_key_code("ctrl"), Some(Code::ControlLeft));
        assert_eq!(parse_key_code("cmd"), Some(Code::MetaLeft));
        assert_eq!(parse_key_code("space"), Some(Code::Space));
        assert_eq!(parse_key_code(" "), Some(Code::Space));
        assert_eq!(parse_key_code("unknown"), None);
    }

    #[test]
    fn test_code_from_char() {
        assert_eq!(code_from_char('a'), Some((Code::KeyA, false)));
        assert_eq!(code_from_char('A'), Some((Code::KeyA, true)));
        assert_eq!(code_from_char('1'), Some((Code::Digit1, false)));
        assert_eq!(code_from_char('!'), Some((Code::Digit1, true)));
        assert_eq!(code_from_char(' '), Some((Code::Space, false)));
    }

    #[test]
    fn test_parse_modifiers() {
        let mods = parse_modifiers("ctrl+shift");
        assert!(mods.contains(Modifiers::CONTROL));
        assert!(mods.contains(Modifiers::SHIFT));
        assert!(!mods.contains(Modifiers::ALT));
        assert!(!mods.contains(Modifiers::META));

        let mods = parse_modifiers("cmd");
        assert!(mods.contains(Modifiers::META));
        assert!(!mods.contains(Modifiers::SHIFT));
    }
}
