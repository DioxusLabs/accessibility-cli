//! Mapping text to simulator key presses.
//!
//! The simulator's keyboard takes USB HID usage codes and has no notion of a
//! shift flag, so a capital letter or a shifted symbol is a *sequence*: hold
//! Left Shift, press the base key, release both. This module owns that
//! translation so the browser does not have to duplicate the table.
//!
//! Scope is deliberately US ASCII. There is no layout awareness and no
//! Unicode: a character outside the table is reported as an error rather than
//! silently dropped or turned into the wrong key, which is how the previous
//! implementation lost every `@` and quietly lowercased every capital.

use anyhow::{Result, anyhow};

/// USB HID keyboard usage codes, re-exported for the input layer.
pub mod usage {
    pub const RETURN: u32 = 40;
    pub const ESCAPE: u32 = 41;
    pub const BACKSPACE: u32 = 42;
    pub const TAB: u32 = 43;
    pub const RIGHT_ARROW: u32 = 79;
    pub const LEFT_ARROW: u32 = 80;
    pub const DOWN_ARROW: u32 = 81;
    pub const UP_ARROW: u32 = 82;
    pub const LEFT_CONTROL: u32 = 224;
    pub const LEFT_SHIFT: u32 = 225;
    pub const LEFT_ALT: u32 = 226;
    pub const LEFT_GUI: u32 = 227;
}

/// A single key press: the usage code and whether Shift is held for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyStroke {
    pub usage: u32,
    pub shift: bool,
}

impl KeyStroke {
    const fn plain(usage: u32) -> Self {
        Self {
            usage,
            shift: false,
        }
    }

    const fn shifted(usage: u32) -> Self {
        Self { usage, shift: true }
    }

    /// The modifier usages to hold while pressing this key.
    pub fn modifiers(&self) -> Vec<u32> {
        if self.shift {
            vec![usage::LEFT_SHIFT]
        } else {
            Vec::new()
        }
    }
}

/// Symbols reachable without Shift, in USB HID usage order.
const UNSHIFTED_SYMBOLS: &[(char, u32)] = &[
    ('-', 45),
    ('=', 46),
    ('[', 47),
    (']', 48),
    ('\\', 49),
    (';', 51),
    ('\'', 52),
    ('`', 53),
    (',', 54),
    ('.', 55),
    ('/', 56),
];

/// Symbols produced by holding Shift over another key's usage.
const SHIFTED_SYMBOLS: &[(char, u32)] = &[
    ('!', 30),
    ('@', 31),
    ('#', 32),
    ('$', 33),
    ('%', 34),
    ('^', 35),
    ('&', 36),
    ('*', 37),
    ('(', 38),
    (')', 39),
    ('_', 45),
    ('+', 46),
    ('{', 47),
    ('}', 48),
    ('|', 49),
    (':', 51),
    ('"', 52),
    ('~', 53),
    ('<', 54),
    ('>', 55),
    ('?', 56),
];

/// The key press that produces `character`, if one exists on a US keyboard.
pub fn keystroke_for(character: char) -> Option<KeyStroke> {
    match character {
        // Letters share a usage; case is the Shift modifier.
        'a'..='z' => Some(KeyStroke::plain(character as u32 - 'a' as u32 + 4)),
        'A'..='Z' => Some(KeyStroke::shifted(character as u32 - 'A' as u32 + 4)),
        // Digits are not contiguous with zero: 1-9 are 30-38 and 0 is 39.
        '1'..='9' => Some(KeyStroke::plain(character as u32 - '1' as u32 + 30)),
        '0' => Some(KeyStroke::plain(39)),
        '\n' | '\r' => Some(KeyStroke::plain(usage::RETURN)),
        '\t' => Some(KeyStroke::plain(usage::TAB)),
        ' ' => Some(KeyStroke::plain(44)),
        _ => UNSHIFTED_SYMBOLS
            .iter()
            .find(|(c, _)| *c == character)
            .map(|(_, usage)| KeyStroke::plain(*usage))
            .or_else(|| {
                SHIFTED_SYMBOLS
                    .iter()
                    .find(|(c, _)| *c == character)
                    .map(|(_, usage)| KeyStroke::shifted(*usage))
            }),
    }
}

/// Translate a string into key presses.
///
/// Fails on the first unmappable character rather than typing a partial or
/// wrong string.
pub fn keystrokes_for(text: &str) -> Result<Vec<KeyStroke>> {
    text.chars()
        .map(|character| {
            keystroke_for(character).ok_or_else(|| {
                anyhow!(
                    "cannot type {character:?}: only US-ASCII characters are supported \
                     (no unicode or emoji)"
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_text(text: &str) -> Vec<(u32, bool)> {
        keystrokes_for(text)
            .expect("typeable")
            .into_iter()
            .map(|k| (k.usage, k.shift))
            .collect()
    }

    #[test]
    fn letters_use_usb_hid_usages_not_hitoolbox() {
        // The bug this module exists to fix: 'a' is usage 4, not HIToolbox 0.
        assert_eq!(type_text("a"), vec![(4, false)]);
        assert_eq!(type_text("b"), vec![(5, false)]);
        assert_eq!(type_text("z"), vec![(29, false)]);
    }

    #[test]
    fn capitals_hold_shift_over_the_same_usage() {
        assert_eq!(type_text("aA"), vec![(4, false), (4, true)]);
    }

    #[test]
    fn digits_are_not_contiguous_with_zero() {
        assert_eq!(type_text("1"), vec![(30, false)]);
        assert_eq!(type_text("9"), vec![(38, false)]);
        assert_eq!(type_text("0"), vec![(39, false)]);
    }

    #[test]
    fn types_an_email_address() {
        // The exact string the old implementation mangled into
        // "testexample.com": @ and ? dropped, capitals lowercased.
        let strokes = type_text("Test@Example.com?");
        assert_eq!(strokes.len(), 17, "every character must produce a key");
        assert_eq!(strokes[0], (23, true), "T is shifted t");
        assert_eq!(strokes[4], (31, true), "@ is shift-2");
        assert_eq!(strokes[16], (56, true), "? is shift-/");
    }

    #[test]
    fn shifted_and_unshifted_symbols_share_usages() {
        assert_eq!(type_text(";"), vec![(51, false)]);
        assert_eq!(type_text(":"), vec![(51, true)]);
        assert_eq!(type_text("/"), vec![(56, false)]);
        assert_eq!(type_text("?"), vec![(56, true)]);
    }

    #[test]
    fn whitespace_maps_to_real_keys() {
        assert_eq!(type_text(" "), vec![(44, false)]);
        assert_eq!(type_text("\n"), vec![(usage::RETURN, false)]);
        assert_eq!(type_text("\t"), vec![(usage::TAB, false)]);
    }

    #[test]
    fn unmappable_characters_fail_loudly() {
        // Better a clear error than a silently wrong string.
        for text in ["café", "hello 👋", "→"] {
            let error = keystrokes_for(text).expect_err("should reject");
            assert!(error.to_string().contains("cannot type"), "{error}");
        }
    }

    #[test]
    fn failure_is_reported_before_anything_is_typed() {
        // keystrokes_for collects into a Result, so a bad character partway
        // through yields no partial output for the caller to send.
        assert!(keystrokes_for("ok\u{1F600}bad").is_err());
    }

    #[test]
    fn shift_is_the_only_modifier_for_text() {
        assert_eq!(
            keystroke_for('A').expect("mapped").modifiers(),
            vec![usage::LEFT_SHIFT]
        );
        assert!(keystroke_for('a').expect("mapped").modifiers().is_empty());
    }

    #[test]
    fn every_printable_ascii_character_is_typeable() {
        // If this ever regresses, some character silently becomes untypeable.
        for byte in 0x20u8..0x7F {
            let character = byte as char;
            assert!(
                keystroke_for(character).is_some(),
                "no key for {character:?}"
            );
        }
    }
}
