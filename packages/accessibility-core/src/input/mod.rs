//! Raw input injection types for computer use AI.
//!
//! This module provides types and utilities for keyboard and mouse input:
//! - Key codes (from keyboard_types)
//! - Modifiers (Ctrl, Shift, Alt, Meta)
//! - Mouse buttons
//! - Character to key code mapping
//!
//! Input injection is performed via the `AccessibilityReader` trait's input methods:
//! - `keystroke` / `press_key` / `release_key` for keyboard input
//! - `type_raw` for typing text
//! - `mouse_click` / `mouse_click_at` / `mouse_move` / `mouse_scroll` for mouse input

mod types;

pub use types::*;
