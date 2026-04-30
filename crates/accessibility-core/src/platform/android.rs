//! Android device/emulator accessibility via ADB.
//!
//! This module provides accessibility support for Android devices and emulators through
//! the Android Debug Bridge (ADB). Unlike other platforms that use native accessibility APIs,
//! Android support works via shell commands executed through ADB.
//!
//! # Architecture
//!
//! ```text
//! Rust (AndroidAccessibility)
//!     ↓ std::process::Command
//! adb shell / adb exec-out
//!     ↓
//! Android Device/Emulator
//! ```
//!
//! # Key Differences from Other Platforms
//!
//! - **Cross-platform host support**: Works on macOS, Windows, and Linux hosts (wherever ADB is available)
//! - **No native accessibility API**: Uses `uiautomator dump` for UI tree and `input` commands for actions
//! - **No event listening**: Android via ADB doesn't support real-time accessibility events
//!
//! # Example
//!
//! ```ignore
//! use accessibility_core::platform::android::AndroidAccessibility;
//! use accessibility_core::accessibility::{AccessibilityReader, TreeFilter};
//!
//! // Connect to the default device
//! let mut reader = AndroidAccessibility::new(None)?;
//!
//! // Get the UI tree
//! let tree = reader.get_tree(None, &TreeFilter::default()).await?;
//! println!("{:?}", tree);
//!
//! // Press the back button
//! reader.press_back().await?;
//! ```

use std::future::Future;
use std::process::{Command, Output};

use accesskit::{Action, Role};
use anyhow::{Context, Result, anyhow, bail};
use quick_xml::Reader;
use quick_xml::events::Event;
use slotmap::SecondaryMap;

use crate::accessibility::{
    AccessibilityEvent, AccessibilityEventType, AccessibilityReader, Element, ElementCache,
    ElementKey, ElementTree, ListenerConfig, ListenerHandle, Point, Rect, Screenshot, Size,
    TreeFilter,
};
use crate::input::{Code, Modifiers, MouseButton};

// ============================================================================
// Android Key Codes
// ============================================================================

/// Android key codes for `input keyevent` command.
///
/// These correspond to the KEYCODE_* constants in Android's KeyEvent class.
/// See: https://developer.android.com/reference/android/view/KeyEvent
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum AndroidKeyCode {
    Unknown = 0,
    SoftLeft = 1,
    SoftRight = 2,
    Home = 3,
    Back = 4,
    Call = 5,
    EndCall = 6,
    Digit0 = 7,
    Digit1 = 8,
    Digit2 = 9,
    Digit3 = 10,
    Digit4 = 11,
    Digit5 = 12,
    Digit6 = 13,
    Digit7 = 14,
    Digit8 = 15,
    Digit9 = 16,
    Star = 17,
    Pound = 18,
    DpadUp = 19,
    DpadDown = 20,
    DpadLeft = 21,
    DpadRight = 22,
    DpadCenter = 23,
    VolumeUp = 24,
    VolumeDown = 25,
    Power = 26,
    Camera = 27,
    Clear = 28,
    A = 29,
    B = 30,
    C = 31,
    D = 32,
    E = 33,
    F = 34,
    G = 35,
    H = 36,
    I = 37,
    J = 38,
    K = 39,
    L = 40,
    M = 41,
    N = 42,
    O = 43,
    P = 44,
    Q = 45,
    R = 46,
    S = 47,
    T = 48,
    U = 49,
    V = 50,
    W = 51,
    X = 52,
    Y = 53,
    Z = 54,
    Comma = 55,
    Period = 56,
    AltLeft = 57,
    AltRight = 58,
    ShiftLeft = 59,
    ShiftRight = 60,
    Tab = 61,
    Space = 62,
    Sym = 63,
    Explorer = 64,
    Envelope = 65,
    Enter = 66,
    Del = 67, // Backspace
    Grave = 68,
    Minus = 69,
    Equals = 70,
    LeftBracket = 71,
    RightBracket = 72,
    Backslash = 73,
    Semicolon = 74,
    Apostrophe = 75,
    Slash = 76,
    At = 77,
    Num = 78,
    HeadsetHook = 79,
    Focus = 80,
    Plus = 81,
    Menu = 82,
    Notification = 83,
    Search = 84,
    MediaPlayPause = 85,
    MediaStop = 86,
    MediaNext = 87,
    MediaPrevious = 88,
    MediaRewind = 89,
    MediaFastForward = 90,
    Mute = 91,
    PageUp = 92,
    PageDown = 93,
    PictSymbols = 94,
    SwitchCharset = 95,
    ButtonA = 96,
    ButtonB = 97,
    ButtonC = 98,
    ButtonX = 99,
    ButtonY = 100,
    ButtonZ = 101,
    ButtonL1 = 102,
    ButtonR1 = 103,
    ButtonL2 = 104,
    ButtonR2 = 105,
    ButtonThumbL = 106,
    ButtonThumbR = 107,
    ButtonStart = 108,
    ButtonSelect = 109,
    ButtonMode = 110,
    Escape = 111,
    ForwardDel = 112,
    CtrlLeft = 113,
    CtrlRight = 114,
    CapsLock = 115,
    ScrollLock = 116,
    MetaLeft = 117,
    MetaRight = 118,
    Function = 119,
    SysRq = 120,
    Break = 121,
    MoveHome = 122,
    MoveEnd = 123,
    Insert = 124,
    Forward = 125,
    MediaPlay = 126,
    MediaPause = 127,
    MediaClose = 128,
    MediaEject = 129,
    MediaRecord = 130,
    F1 = 131,
    F2 = 132,
    F3 = 133,
    F4 = 134,
    F5 = 135,
    F6 = 136,
    F7 = 137,
    F8 = 138,
    F9 = 139,
    F10 = 140,
    F11 = 141,
    F12 = 142,
    NumLock = 143,
    Numpad0 = 144,
    Numpad1 = 145,
    Numpad2 = 146,
    Numpad3 = 147,
    Numpad4 = 148,
    Numpad5 = 149,
    Numpad6 = 150,
    Numpad7 = 151,
    Numpad8 = 152,
    Numpad9 = 153,
    NumpadDivide = 154,
    NumpadMultiply = 155,
    NumpadSubtract = 156,
    NumpadAdd = 157,
    NumpadDot = 158,
    NumpadComma = 159,
    NumpadEnter = 160,
    NumpadEquals = 161,
    NumpadLeftParen = 162,
    NumpadRightParen = 163,
    VolumeMute = 164,
    Info = 165,
    ChannelUp = 166,
    ChannelDown = 167,
    ZoomIn = 168,
    ZoomOut = 169,
    Tv = 170,
    Window = 171,
    Guide = 172,
    Dvr = 173,
    Bookmark = 174,
    Captions = 175,
    Settings = 176,
    TvPower = 177,
    TvInput = 178,
    StbPower = 179,
    StbInput = 180,
    AvrPower = 181,
    AvrInput = 182,
    ProgRed = 183,
    ProgGreen = 184,
    ProgYellow = 185,
    ProgBlue = 186,
    AppSwitch = 187, // Recent apps
    Button1 = 188,
    Button2 = 189,
    Button3 = 190,
    Button4 = 191,
    Button5 = 192,
    Button6 = 193,
    Button7 = 194,
    Button8 = 195,
    Button9 = 196,
    Button10 = 197,
    Button11 = 198,
    Button12 = 199,
    Button13 = 200,
    Button14 = 201,
    Button15 = 202,
    Button16 = 203,
    LanguageSwitch = 204,
    MannerMode = 205,
    Mode3d = 206,
    Contacts = 207,
    Calendar = 208,
    Music = 209,
    Calculator = 210,
    ZenkakuHankaku = 211,
    Eisu = 212,
    Muhenkan = 213,
    Henkan = 214,
    KatakanaHiragana = 215,
    Yen = 216,
    Ro = 217,
    Kana = 218,
    Assist = 219,
    BrightnessDown = 220,
    BrightnessUp = 221,
    MediaAudioTrack = 222,
    Sleep = 223,
    Wakeup = 224,
    Pairing = 225,
    MediaTopMenu = 226,
    Digit11 = 227,
    Digit12 = 228,
    LastChannel = 229,
    TvDataService = 230,
    VoiceAssist = 231,
    TvRadioService = 232,
    TvTeletext = 233,
    TvNumberEntry = 234,
    TvTerrestrialAnalog = 235,
    TvTerrestrialDigital = 236,
    TvSatellite = 237,
    TvSatelliteBs = 238,
    TvSatelliteCs = 239,
    TvSatelliteService = 240,
    TvNetwork = 241,
    TvAntennaCable = 242,
    TvInputHdmi1 = 243,
    TvInputHdmi2 = 244,
    TvInputHdmi3 = 245,
    TvInputHdmi4 = 246,
    TvInputComposite1 = 247,
    TvInputComposite2 = 248,
    TvInputComponent1 = 249,
    TvInputComponent2 = 250,
    TvInputVga1 = 251,
    TvAudioDescription = 252,
    TvAudioDescriptionMixUp = 253,
    TvAudioDescriptionMixDown = 254,
    TvZoomMode = 255,
    TvContentsMenu = 256,
    TvMediaContextMenu = 257,
    TvTimerProgramming = 258,
    Help = 259,
    NavigatePrevious = 260,
    NavigateNext = 261,
    NavigateIn = 262,
    NavigateOut = 263,
    StemPrimary = 264,
    Stem1 = 265,
    Stem2 = 266,
    Stem3 = 267,
    DpadUpLeft = 268,
    DpadDownLeft = 269,
    DpadUpRight = 270,
    DpadDownRight = 271,
    MediaSkipForward = 272,
    MediaSkipBackward = 273,
    MediaStepForward = 274,
    MediaStepBackward = 275,
    SoftSleep = 276,
    Cut = 277,
    Copy = 278,
    Paste = 279,
    SystemNavigationUp = 280,
    SystemNavigationDown = 281,
    SystemNavigationLeft = 282,
    SystemNavigationRight = 283,
    AllApps = 284,
    Refresh = 285,
    ThumbsUp = 286,
    ThumbsDown = 287,
    ProfileSwitch = 288,
}

impl AndroidKeyCode {
    /// Convert a keyboard-types Code to an Android key code.
    pub fn from_code(code: Code) -> Option<Self> {
        Some(match code {
            // Letters
            Code::KeyA => AndroidKeyCode::A,
            Code::KeyB => AndroidKeyCode::B,
            Code::KeyC => AndroidKeyCode::C,
            Code::KeyD => AndroidKeyCode::D,
            Code::KeyE => AndroidKeyCode::E,
            Code::KeyF => AndroidKeyCode::F,
            Code::KeyG => AndroidKeyCode::G,
            Code::KeyH => AndroidKeyCode::H,
            Code::KeyI => AndroidKeyCode::I,
            Code::KeyJ => AndroidKeyCode::J,
            Code::KeyK => AndroidKeyCode::K,
            Code::KeyL => AndroidKeyCode::L,
            Code::KeyM => AndroidKeyCode::M,
            Code::KeyN => AndroidKeyCode::N,
            Code::KeyO => AndroidKeyCode::O,
            Code::KeyP => AndroidKeyCode::P,
            Code::KeyQ => AndroidKeyCode::Q,
            Code::KeyR => AndroidKeyCode::R,
            Code::KeyS => AndroidKeyCode::S,
            Code::KeyT => AndroidKeyCode::T,
            Code::KeyU => AndroidKeyCode::U,
            Code::KeyV => AndroidKeyCode::V,
            Code::KeyW => AndroidKeyCode::W,
            Code::KeyX => AndroidKeyCode::X,
            Code::KeyY => AndroidKeyCode::Y,
            Code::KeyZ => AndroidKeyCode::Z,

            // Digits
            Code::Digit0 => AndroidKeyCode::Digit0,
            Code::Digit1 => AndroidKeyCode::Digit1,
            Code::Digit2 => AndroidKeyCode::Digit2,
            Code::Digit3 => AndroidKeyCode::Digit3,
            Code::Digit4 => AndroidKeyCode::Digit4,
            Code::Digit5 => AndroidKeyCode::Digit5,
            Code::Digit6 => AndroidKeyCode::Digit6,
            Code::Digit7 => AndroidKeyCode::Digit7,
            Code::Digit8 => AndroidKeyCode::Digit8,
            Code::Digit9 => AndroidKeyCode::Digit9,

            // Function keys
            Code::F1 => AndroidKeyCode::F1,
            Code::F2 => AndroidKeyCode::F2,
            Code::F3 => AndroidKeyCode::F3,
            Code::F4 => AndroidKeyCode::F4,
            Code::F5 => AndroidKeyCode::F5,
            Code::F6 => AndroidKeyCode::F6,
            Code::F7 => AndroidKeyCode::F7,
            Code::F8 => AndroidKeyCode::F8,
            Code::F9 => AndroidKeyCode::F9,
            Code::F10 => AndroidKeyCode::F10,
            Code::F11 => AndroidKeyCode::F11,
            Code::F12 => AndroidKeyCode::F12,

            // Navigation
            Code::ArrowUp => AndroidKeyCode::DpadUp,
            Code::ArrowDown => AndroidKeyCode::DpadDown,
            Code::ArrowLeft => AndroidKeyCode::DpadLeft,
            Code::ArrowRight => AndroidKeyCode::DpadRight,
            Code::Home => AndroidKeyCode::MoveHome,
            Code::End => AndroidKeyCode::MoveEnd,
            Code::PageUp => AndroidKeyCode::PageUp,
            Code::PageDown => AndroidKeyCode::PageDown,

            // Editing
            Code::Enter => AndroidKeyCode::Enter,
            Code::NumpadEnter => AndroidKeyCode::NumpadEnter,
            Code::Backspace => AndroidKeyCode::Del,
            Code::Delete => AndroidKeyCode::ForwardDel,
            Code::Insert => AndroidKeyCode::Insert,
            Code::Tab => AndroidKeyCode::Tab,
            Code::Escape => AndroidKeyCode::Escape,
            Code::Space => AndroidKeyCode::Space,

            // Modifiers
            Code::ShiftLeft => AndroidKeyCode::ShiftLeft,
            Code::ShiftRight => AndroidKeyCode::ShiftRight,
            Code::ControlLeft => AndroidKeyCode::CtrlLeft,
            Code::ControlRight => AndroidKeyCode::CtrlRight,
            Code::AltLeft => AndroidKeyCode::AltLeft,
            Code::AltRight => AndroidKeyCode::AltRight,
            Code::MetaLeft => AndroidKeyCode::MetaLeft,
            Code::MetaRight => AndroidKeyCode::MetaRight,
            Code::CapsLock => AndroidKeyCode::CapsLock,
            Code::NumLock => AndroidKeyCode::NumLock,
            Code::ScrollLock => AndroidKeyCode::ScrollLock,

            // Punctuation
            Code::Comma => AndroidKeyCode::Comma,
            Code::Period => AndroidKeyCode::Period,
            Code::Slash => AndroidKeyCode::Slash,
            Code::Semicolon => AndroidKeyCode::Semicolon,
            Code::Quote => AndroidKeyCode::Apostrophe,
            Code::BracketLeft => AndroidKeyCode::LeftBracket,
            Code::BracketRight => AndroidKeyCode::RightBracket,
            Code::Backslash => AndroidKeyCode::Backslash,
            Code::Minus => AndroidKeyCode::Minus,
            Code::Equal => AndroidKeyCode::Equals,
            Code::Backquote => AndroidKeyCode::Grave,

            // Numpad
            Code::Numpad0 => AndroidKeyCode::Numpad0,
            Code::Numpad1 => AndroidKeyCode::Numpad1,
            Code::Numpad2 => AndroidKeyCode::Numpad2,
            Code::Numpad3 => AndroidKeyCode::Numpad3,
            Code::Numpad4 => AndroidKeyCode::Numpad4,
            Code::Numpad5 => AndroidKeyCode::Numpad5,
            Code::Numpad6 => AndroidKeyCode::Numpad6,
            Code::Numpad7 => AndroidKeyCode::Numpad7,
            Code::Numpad8 => AndroidKeyCode::Numpad8,
            Code::Numpad9 => AndroidKeyCode::Numpad9,
            Code::NumpadAdd => AndroidKeyCode::NumpadAdd,
            Code::NumpadSubtract => AndroidKeyCode::NumpadSubtract,
            Code::NumpadMultiply => AndroidKeyCode::NumpadMultiply,
            Code::NumpadDivide => AndroidKeyCode::NumpadDivide,
            Code::NumpadDecimal => AndroidKeyCode::NumpadDot,

            // Media
            Code::AudioVolumeMute => AndroidKeyCode::VolumeMute,
            Code::AudioVolumeDown => AndroidKeyCode::VolumeDown,
            Code::AudioVolumeUp => AndroidKeyCode::VolumeUp,
            Code::MediaPlayPause => AndroidKeyCode::MediaPlayPause,
            Code::MediaStop => AndroidKeyCode::MediaStop,
            Code::MediaTrackNext => AndroidKeyCode::MediaNext,
            Code::MediaTrackPrevious => AndroidKeyCode::MediaPrevious,

            _ => return None,
        })
    }
}

// ============================================================================
// ADB Client
// ============================================================================

/// ADB command execution wrapper.
///
/// Provides methods to execute ADB commands with optional device targeting.
#[derive(Debug, Clone)]
pub struct AdbClient {
    /// Device serial number for multi-device scenarios (from `adb devices`).
    pub serial: Option<String>,
    /// Path to the ADB binary.
    pub adb_path: String,
}

impl Default for AdbClient {
    fn default() -> Self {
        Self {
            serial: None,
            adb_path: "adb".to_string(),
        }
    }
}

impl AdbClient {
    /// Create a new ADB client.
    ///
    /// # Arguments
    /// * `serial` - Optional device serial number (use `adb devices` to list).
    ///              If None, uses the default (only) connected device.
    pub fn new(serial: Option<&str>) -> Self {
        Self {
            serial: serial.map(String::from),
            adb_path: "adb".to_string(),
        }
    }

    /// Create a new ADB client with a custom ADB path.
    pub fn with_adb_path(serial: Option<&str>, adb_path: &str) -> Self {
        Self {
            serial: serial.map(String::from),
            adb_path: adb_path.to_string(),
        }
    }

    /// Build base ADB command with optional device serial.
    fn base_command(&self) -> Command {
        let mut cmd = Command::new(&self.adb_path);
        if let Some(ref serial) = self.serial {
            cmd.arg("-s").arg(serial);
        }
        cmd
    }

    /// Execute an ADB shell command.
    ///
    /// Runs `adb shell <args>` and returns stdout.
    pub fn shell(&self, args: &[&str]) -> Result<String> {
        let mut cmd = self.base_command();
        cmd.arg("shell").args(args);

        let output = cmd
            .output()
            .context("Failed to execute adb shell command")?;

        Self::check_output(&output, "shell")?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Execute an ADB shell command and return raw bytes.
    ///
    /// Useful for binary data like screenshots.
    pub fn shell_raw(&self, args: &[&str]) -> Result<Vec<u8>> {
        let mut cmd = self.base_command();
        cmd.arg("shell").args(args);

        let output = cmd
            .output()
            .context("Failed to execute adb shell command")?;

        Self::check_output(&output, "shell")?;
        Ok(output.stdout)
    }

    /// Execute `adb exec-out` for efficient binary output.
    ///
    /// Unlike `shell`, this doesn't add LF->CRLF conversion on Windows.
    pub fn exec_out(&self, args: &[&str]) -> Result<Vec<u8>> {
        let mut cmd = self.base_command();
        cmd.arg("exec-out").args(args);

        let output = cmd
            .output()
            .context("Failed to execute adb exec-out command")?;

        Self::check_output(&output, "exec-out")?;
        Ok(output.stdout)
    }

    /// Execute a general ADB command (not shell).
    pub fn command(&self, args: &[&str]) -> Result<String> {
        let mut cmd = self.base_command();
        cmd.args(args);

        let output = cmd.output().context("Failed to execute adb command")?;

        Self::check_output(&output, "adb")?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Check command output for errors.
    fn check_output(output: &Output, cmd_type: &str) -> Result<()> {
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            bail!(
                "ADB {} command failed (exit code {}): stdout={}, stderr={}",
                cmd_type,
                output.status.code().unwrap_or(-1),
                stdout.trim(),
                stderr.trim()
            );
        }
        Ok(())
    }

    /// Check if ADB is available and a device is connected.
    pub fn check_connection(&self) -> Result<()> {
        // First check if adb binary exists
        let version_result = Command::new(&self.adb_path).arg("version").output();

        match version_result {
            Ok(output) if output.status.success() => {}
            Ok(_) => bail!("ADB binary found but returned error"),
            Err(e) => bail!(
                "ADB binary not found at '{}': {}. Install Android SDK Platform Tools.",
                self.adb_path,
                e
            ),
        }

        // Check for connected devices
        let devices = self.command(&["devices"])?;
        let device_count = devices
            .lines()
            .skip(1) // Skip header "List of devices attached"
            .filter(|line| {
                let trimmed = line.trim();
                !trimmed.is_empty() && trimmed.contains('\t')
            })
            .count();

        if device_count == 0 {
            bail!("No Android devices connected. Connect a device or start an emulator.");
        }

        // If serial is specified, verify it exists
        if let Some(ref serial) = self.serial {
            let found = devices.lines().skip(1).any(|line| line.starts_with(serial));
            if !found {
                bail!(
                    "Device '{}' not found. Available devices:\n{}",
                    serial,
                    devices
                );
            }
        }

        Ok(())
    }

    /// Get the screen size in pixels.
    pub fn get_screen_size(&self) -> Result<(u32, u32)> {
        let output = self.shell(&["wm", "size"])?;
        // Output format: "Physical size: 1080x1920"
        for line in output.lines() {
            if let Some(size_str) = line.strip_prefix("Physical size:") {
                let size_str = size_str.trim();
                let parts: Vec<&str> = size_str.split('x').collect();
                if parts.len() == 2 {
                    let width = parts[0]
                        .parse::<u32>()
                        .context("Failed to parse screen width")?;
                    let height = parts[1]
                        .parse::<u32>()
                        .context("Failed to parse screen height")?;
                    return Ok((width, height));
                }
            }
        }
        bail!("Failed to parse screen size from: {}", output);
    }

    /// Capture a screenshot as PNG bytes.
    pub fn screenshot(&self) -> Result<Vec<u8>> {
        // Use exec-out for binary data without line ending conversion
        self.exec_out(&["screencap", "-p"])
    }

    /// Tap at screen coordinates.
    pub fn tap(&self, x: f64, y: f64) -> Result<()> {
        self.shell(&[
            "input",
            "tap",
            &x.round().to_string(),
            &y.round().to_string(),
        ])?;
        Ok(())
    }

    /// Swipe from one point to another.
    ///
    /// # Arguments
    /// * `start` - Starting coordinates (x, y)
    /// * `end` - Ending coordinates (x, y)
    /// * `duration_ms` - Duration of the swipe in milliseconds
    pub fn swipe(&self, start: (f64, f64), end: (f64, f64), duration_ms: u64) -> Result<()> {
        self.shell(&[
            "input",
            "swipe",
            &start.0.round().to_string(),
            &start.1.round().to_string(),
            &end.0.round().to_string(),
            &end.1.round().to_string(),
            &duration_ms.to_string(),
        ])?;
        Ok(())
    }

    /// Send a key event.
    pub fn key_event(&self, keycode: u32) -> Result<()> {
        self.shell(&["input", "keyevent", &keycode.to_string()])?;
        Ok(())
    }

    /// Send text input.
    ///
    /// Note: Special characters are escaped for shell safety.
    pub fn input_text(&self, text: &str) -> Result<()> {
        // Escape special characters for shell
        let escaped = escape_shell_text(text);
        self.shell(&["input", "text", &escaped])?;
        Ok(())
    }

    /// Dump the UI hierarchy as XML.
    pub fn dump_ui(&self) -> Result<String> {
        // uiautomator dump outputs XML to a file, but we can use /dev/tty to get it directly
        // Note: Some devices require writing to a file first
        let result = self.shell(&["uiautomator", "dump", "/dev/tty"]);

        match result {
            Ok(xml) => {
                // The output may contain "UI hierarchy dumped to: /dev/tty" followed by the XML
                // Find the XML start
                if let Some(start) = xml.find("<?xml") {
                    Ok(xml[start..].to_string())
                } else if let Some(start) = xml.find("<hierarchy") {
                    Ok(xml[start..].to_string())
                } else {
                    // Try dumping to a file and reading it
                    self.dump_ui_via_file()
                }
            }
            Err(_) => self.dump_ui_via_file(),
        }
    }

    /// Dump UI via temporary file (fallback for devices that don't support /dev/tty).
    fn dump_ui_via_file(&self) -> Result<String> {
        let tmp_path = "/sdcard/window_dump.xml";

        // Dump to file
        self.shell(&["uiautomator", "dump", tmp_path])?;

        // Read the file
        let xml = self.shell(&["cat", tmp_path])?;

        // Clean up
        let _ = self.shell(&["rm", tmp_path]);

        // Find the XML start
        if let Some(start) = xml.find("<?xml") {
            Ok(xml[start..].to_string())
        } else if let Some(start) = xml.find("<hierarchy") {
            Ok(xml[start..].to_string())
        } else {
            bail!(
                "Failed to parse UI dump XML: {}",
                &xml[..xml.len().min(200)]
            );
        }
    }

    /// Launch an app by package name and optional activity.
    ///
    /// If activity is None, launches the main/launcher activity.
    pub fn launch_app(&self, package: &str, activity: Option<&str>) -> Result<()> {
        match activity {
            Some(act) => {
                let component = format!("{}/{}", package, act);
                self.shell(&["am", "start", "-n", &component])?;
            }
            None => {
                // Launch using monkey to start the main activity
                self.shell(&[
                    "monkey",
                    "-p",
                    package,
                    "-c",
                    "android.intent.category.LAUNCHER",
                    "1",
                ])?;
            }
        }
        Ok(())
    }

    /// Force stop an app.
    pub fn stop_app(&self, package: &str) -> Result<()> {
        self.shell(&["am", "force-stop", package])?;
        Ok(())
    }

    /// Get the current foreground activity.
    pub fn get_current_activity(&self) -> Result<String> {
        // Different Android versions have different commands
        let output = self.shell(&["dumpsys", "activity", "activities"])?;

        // Look for "mResumedActivity" or "mFocusedActivity"
        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("mResumedActivity:") || trimmed.starts_with("mFocusedActivity:")
            {
                return Ok(trimmed.to_string());
            }
        }

        // Fallback: try the old method
        let output = self.shell(&["dumpsys", "window", "windows"])?;
        for line in output.lines() {
            if line.contains("mCurrentFocus") || line.contains("mFocusedApp") {
                return Ok(line.trim().to_string());
            }
        }

        bail!("Could not determine current activity");
    }
}

/// Escape text for ADB shell input command.
///
/// ADB input text has issues with special characters, so we escape them.
fn escape_shell_text(text: &str) -> String {
    let mut result = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        match c {
            // Characters that need escaping in shell
            ' ' => result.push_str("%s"),
            '\'' => result.push_str("'\"'\"'"),
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '`' => result.push_str("\\`"),
            '$' => result.push_str("\\$"),
            '&' => result.push_str("\\&"),
            '|' => result.push_str("\\|"),
            ';' => result.push_str("\\;"),
            '<' => result.push_str("\\<"),
            '>' => result.push_str("\\>"),
            '(' => result.push_str("\\("),
            ')' => result.push_str("\\)"),
            '[' => result.push_str("\\["),
            ']' => result.push_str("\\]"),
            '{' => result.push_str("\\{"),
            '}' => result.push_str("\\}"),
            '!' => result.push_str("\\!"),
            '#' => result.push_str("\\#"),
            '*' => result.push_str("\\*"),
            '?' => result.push_str("\\?"),
            '~' => result.push_str("\\~"),
            _ => result.push(c),
        }
    }
    result
}

// ============================================================================
// XML Parsing and Role Mapping
// ============================================================================

/// Parse Android bounds string like "[0,0][1080,1920]" into a Rect.
fn parse_bounds(bounds_str: &str) -> Option<Rect> {
    // Format: "[left,top][right,bottom]"
    let trimmed = bounds_str.trim();
    if !trimmed.starts_with('[') || !trimmed.contains("][") {
        return None;
    }

    // Split into two coordinate pairs
    let parts: Vec<&str> = trimmed.split("][").collect();
    if parts.len() != 2 {
        return None;
    }

    // Parse first pair (left, top)
    let first = parts[0].trim_start_matches('[');
    let first_coords: Vec<&str> = first.split(',').collect();
    if first_coords.len() != 2 {
        return None;
    }

    // Parse second pair (right, bottom)
    let second = parts[1].trim_end_matches(']');
    let second_coords: Vec<&str> = second.split(',').collect();
    if second_coords.len() != 2 {
        return None;
    }

    let left: f64 = first_coords[0].parse().ok()?;
    let top: f64 = first_coords[1].parse().ok()?;
    let right: f64 = second_coords[0].parse().ok()?;
    let bottom: f64 = second_coords[1].parse().ok()?;

    Some(Rect::new(
        Point::new(left, top),
        Size::new(right - left, bottom - top),
    ))
}

/// Map Android class name to AccessKit Role.
fn map_android_class_to_role(class: &str) -> Role {
    // Extract the simple class name (last part after dot)
    let simple_name = class.rsplit('.').next().unwrap_or(class);

    match simple_name {
        // Buttons
        "Button"
        | "ImageButton"
        | "FloatingActionButton"
        | "MaterialButton"
        | "AppCompatButton" => Role::Button,

        // Text inputs
        "EditText"
        | "AutoCompleteTextView"
        | "MultiAutoCompleteTextView"
        | "TextInputEditText"
        | "AppCompatEditText" => Role::TextInput,

        // Text display
        "TextView" | "AppCompatTextView" => Role::Label,

        // Checkboxes and toggles
        "CheckBox" | "AppCompatCheckBox" | "MaterialCheckBox" => Role::CheckBox,
        "Switch" | "SwitchCompat" | "SwitchMaterial" => Role::Switch,
        "ToggleButton" => Role::Switch,
        "RadioButton" | "AppCompatRadioButton" | "MaterialRadioButton" => Role::RadioButton,

        // Images
        "ImageView" | "AppCompatImageView" => Role::Image,

        // Lists and scrolling
        "ListView" => Role::List,
        "RecyclerView" => Role::List,
        "GridView" => Role::Grid,
        "ScrollView" | "HorizontalScrollView" | "NestedScrollView" => Role::ScrollView,

        // Containers
        "LinearLayout" | "RelativeLayout" | "FrameLayout" | "ConstraintLayout"
        | "CoordinatorLayout" | "TableLayout" => Role::GenericContainer,
        "ViewGroup" => Role::GenericContainer,
        "CardView" | "MaterialCardView" => Role::GenericContainer,

        // Tabs
        "TabLayout" | "TabItem" => Role::Tab,
        "ViewPager" | "ViewPager2" => Role::TabList,

        // Navigation
        "NavigationView" | "NavigationRailView" | "BottomNavigationView" => Role::Navigation,
        "Toolbar" | "ActionBar" | "MaterialToolbar" => Role::Toolbar,

        // Dialogs
        "AlertDialog" | "Dialog" => Role::Dialog,

        // Progress indicators
        "ProgressBar" | "CircularProgressIndicator" | "LinearProgressIndicator" => {
            Role::ProgressIndicator
        }

        // Sliders
        "SeekBar" | "RatingBar" | "Slider" => Role::Slider,

        // Spinners (dropdowns)
        "Spinner" | "AppCompatSpinner" => Role::ComboBox,

        // Web views
        "WebView" => Role::Document,

        // Menu items
        "MenuItem" => Role::MenuItem,

        // Links
        "URLSpan" => Role::Link,

        // Default for unknown classes
        _ => {
            // Check for common patterns
            if simple_name.contains("Button") {
                Role::Button
            } else if simple_name.contains("Text") && simple_name.contains("Edit") {
                Role::TextInput
            } else if simple_name.contains("Text") {
                Role::Label
            } else if simple_name.contains("Check") {
                Role::CheckBox
            } else if simple_name.contains("Radio") {
                Role::RadioButton
            } else if simple_name.contains("Image") {
                Role::Image
            } else if simple_name.contains("List") {
                Role::List
            } else if simple_name.contains("Scroll") {
                Role::ScrollView
            } else if simple_name.contains("Layout") || simple_name.contains("Container") {
                Role::GenericContainer
            } else if simple_name.contains("Dialog") {
                Role::Dialog
            } else if simple_name.contains("Progress") {
                Role::ProgressIndicator
            } else if simple_name.contains("Seek") || simple_name.contains("Slider") {
                Role::Slider
            } else if simple_name.contains("Spinner") {
                Role::ComboBox
            } else if simple_name.contains("Tab") {
                Role::Tab
            } else if simple_name.contains("Menu") {
                Role::MenuItem
            } else {
                Role::Unknown
            }
        }
    }
}

/// Parsed node from uiautomator XML.
#[derive(Debug)]
struct UiNode {
    /// Android class name (e.g., "android.widget.Button").
    class: String,
    /// Text content.
    text: Option<String>,
    /// Resource ID (e.g., "com.example:id/button").
    resource_id: Option<String>,
    /// Content description (accessibility label).
    content_desc: Option<String>,
    /// Bounds string.
    bounds_str: String,
    /// Whether the node is clickable.
    clickable: bool,
    /// Whether the node is focusable.
    focusable: bool,
    /// Whether the node is enabled.
    enabled: bool,
    /// Whether the node is focused.
    focused: bool,
    /// Whether the node is checkable.
    checkable: bool,
    /// Whether the node is checked.
    checked: bool,
    /// Whether the node is scrollable.
    scrollable: bool,
    /// Whether the node is long-clickable.
    long_clickable: bool,
    /// Package name.
    package: Option<String>,
    /// Child nodes.
    children: Vec<UiNode>,
}

impl UiNode {
    fn new() -> Self {
        Self {
            class: String::new(),
            text: None,
            resource_id: None,
            content_desc: None,
            bounds_str: String::new(),
            clickable: false,
            focusable: false,
            enabled: true,
            focused: false,
            checkable: false,
            checked: false,
            scrollable: false,
            long_clickable: false,
            package: None,
            children: Vec::new(),
        }
    }

    /// Convert to Element, recursively processing children.
    fn to_element(
        &self,
        cache: &mut ElementCache,
        bounds_map: &mut SecondaryMap<ElementKey, String>,
    ) -> Element {
        let role = map_android_class_to_role(&self.class);
        let bounds = parse_bounds(&self.bounds_str);

        let (id, mut element) = cache.store_with_clone(|id| {
            let mut elem = Element::new(id, role);
            elem.title = self
                .text
                .clone()
                .filter(|s| !s.is_empty())
                .or_else(|| self.content_desc.clone().filter(|s| !s.is_empty()));
            elem.description = self.content_desc.clone().filter(|s| !s.is_empty());
            elem.identifier = self.resource_id.clone();
            elem.bounds = bounds;
            elem.enabled = self.enabled;
            elem.focused = self.focused;

            // Build actions list
            let mut actions = Vec::new();
            if self.clickable {
                actions.push("click".to_string());
            }
            if self.long_clickable {
                actions.push("longClick".to_string());
            }
            if self.scrollable {
                actions.push("scroll".to_string());
            }
            if self.checkable {
                actions.push("toggle".to_string());
            }
            if self.focusable {
                actions.push("focus".to_string());
            }
            elem.actions = actions;

            // Store value for checkable items
            if self.checkable {
                elem.value = Some(if self.checked { "true" } else { "false" }.to_string());
            }

            elem
        });

        // Store bounds string for later tap coordinate calculation
        bounds_map.insert(id, self.bounds_str.clone());

        // Process children
        for child in &self.children {
            element.children.push(child.to_element(cache, bounds_map));
        }

        element
    }
}

/// Parse uiautomator XML dump into a tree of UiNodes.
fn parse_ui_xml(xml: &str) -> Result<UiNode> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut node_stack: Vec<UiNode> = vec![UiNode::new()]; // Root container
    node_stack[0].class = "hierarchy".to_string();

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(ref e)) => {
                // Self-closing tag like <node ... />
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();

                if tag_name == "node" {
                    let mut node = UiNode::new();

                    for attr in e.attributes().filter_map(|a| a.ok()) {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        let value = String::from_utf8_lossy(&attr.value).to_string();

                        match key.as_str() {
                            "class" => node.class = value,
                            "text" => node.text = Some(value).filter(|s| !s.is_empty()),
                            "resource-id" => {
                                node.resource_id = Some(value).filter(|s| !s.is_empty())
                            }
                            "content-desc" => {
                                node.content_desc = Some(value).filter(|s| !s.is_empty())
                            }
                            "bounds" => node.bounds_str = value,
                            "clickable" => node.clickable = value == "true",
                            "focusable" => node.focusable = value == "true",
                            "enabled" => node.enabled = value == "true",
                            "focused" => node.focused = value == "true",
                            "checkable" => node.checkable = value == "true",
                            "checked" => node.checked = value == "true",
                            "scrollable" => node.scrollable = value == "true",
                            "long-clickable" => node.long_clickable = value == "true",
                            "package" => node.package = Some(value),
                            _ => {}
                        }
                    }

                    // Add to parent
                    if let Some(parent) = node_stack.last_mut() {
                        parent.children.push(node);
                    }
                }
            }
            Ok(Event::Start(ref e)) => {
                // Opening tag like <node ...> with children
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();

                if tag_name == "node" {
                    let mut node = UiNode::new();

                    for attr in e.attributes().filter_map(|a| a.ok()) {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        let value = String::from_utf8_lossy(&attr.value).to_string();

                        match key.as_str() {
                            "class" => node.class = value,
                            "text" => node.text = Some(value).filter(|s| !s.is_empty()),
                            "resource-id" => {
                                node.resource_id = Some(value).filter(|s| !s.is_empty())
                            }
                            "content-desc" => {
                                node.content_desc = Some(value).filter(|s| !s.is_empty())
                            }
                            "bounds" => node.bounds_str = value,
                            "clickable" => node.clickable = value == "true",
                            "focusable" => node.focusable = value == "true",
                            "enabled" => node.enabled = value == "true",
                            "focused" => node.focused = value == "true",
                            "checkable" => node.checkable = value == "true",
                            "checked" => node.checked = value == "true",
                            "scrollable" => node.scrollable = value == "true",
                            "long-clickable" => node.long_clickable = value == "true",
                            "package" => node.package = Some(value),
                            _ => {}
                        }
                    }

                    // Push onto stack to collect children
                    node_stack.push(node);
                } else if tag_name == "hierarchy" {
                    // Parse hierarchy attributes if present
                    for attr in e.attributes().filter_map(|a| a.ok()) {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        let value = String::from_utf8_lossy(&attr.value).to_string();
                        if key == "rotation" {
                            // Could store rotation info if needed
                            let _ = value;
                        }
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if tag_name == "node" {
                    // Pop this node and add to parent
                    if node_stack.len() > 1 {
                        let node = node_stack.pop().unwrap();
                        if let Some(parent) = node_stack.last_mut() {
                            parent.children.push(node);
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => bail!(
                "Error parsing UI XML at position {}: {:?}",
                reader.buffer_position(),
                e
            ),
            _ => {}
        }
        buf.clear();
    }

    // Return the root node
    Ok(node_stack.remove(0))
}

// ============================================================================
// AndroidAccessibility Implementation
// ============================================================================

/// Android accessibility reader using ADB.
///
/// This implementation uses the Android Debug Bridge (ADB) to:
/// - Query the UI hierarchy via `uiautomator dump`
/// - Capture screenshots via `screencap`
/// - Send input events via `input` commands
///
/// Unlike native accessibility APIs on other platforms, Android via ADB:
/// - Works on any host OS (macOS, Linux, Windows) where ADB is available
/// - Has higher latency due to process spawning for each command
/// - Does not support real-time event listening
pub struct AndroidAccessibility {
    /// ADB client for command execution.
    adb: AdbClient,
    /// Element cache for ID management.
    cache: ElementCache,
    /// Map from element key to bounds string (for tap coordinate calculation).
    element_bounds: SecondaryMap<ElementKey, String>,
    /// Cached screen size.
    screen_size: Option<(u32, u32)>,
    /// Last known app package (for PID-like targeting).
    last_package: Option<String>,
}

impl AndroidAccessibility {
    /// Create a new Android accessibility reader.
    ///
    /// # Arguments
    /// * `serial` - Optional device serial number. Use `adb devices` to list connected devices.
    ///              If None, uses the default (only) connected device.
    ///
    /// # Errors
    /// Returns an error if ADB is not available or no device is connected.
    ///
    /// # Example
    /// ```ignore
    /// // Connect to default device
    /// let reader = AndroidAccessibility::new(None)?;
    ///
    /// // Connect to specific device
    /// let reader = AndroidAccessibility::new(Some("emulator-5554"))?;
    /// ```
    pub fn new(serial: Option<&str>) -> Result<Self> {
        let adb = AdbClient::new(serial);
        adb.check_connection()?;

        // Get initial screen size
        let screen_size = adb.get_screen_size().ok();

        Ok(Self {
            adb,
            cache: ElementCache::new(),
            element_bounds: SecondaryMap::new(),
            screen_size,
            last_package: None,
        })
    }

    /// Create a new Android accessibility reader with a custom ADB path.
    pub fn with_adb_path(serial: Option<&str>, adb_path: &str) -> Result<Self> {
        let adb = AdbClient::with_adb_path(serial, adb_path);
        adb.check_connection()?;

        let screen_size = adb.get_screen_size().ok();

        Ok(Self {
            adb,
            cache: ElementCache::new(),
            element_bounds: SecondaryMap::new(),
            screen_size,
            last_package: None,
        })
    }

    /// Get the ADB client for direct command access.
    pub fn adb(&self) -> &AdbClient {
        &self.adb
    }

    /// Get the screen size (width, height) in pixels.
    pub fn screen_size(&self) -> Option<(u32, u32)> {
        self.screen_size
    }

    /// Refresh the cached screen size.
    pub fn refresh_screen_size(&mut self) -> Result<(u32, u32)> {
        let size = self.adb.get_screen_size()?;
        self.screen_size = Some(size);
        Ok(size)
    }

    /// Get the center point of an element by its ID.
    fn get_element_center(&self, id: ElementKey) -> Option<Point> {
        let bounds_str = self.element_bounds.get(id)?;
        let bounds = parse_bounds(bounds_str)?;
        Some(bounds.center())
    }
}

// ============================================================================
// AccessibilityReader Trait Implementation
// ============================================================================

impl AccessibilityReader for AndroidAccessibility {
    fn get_tree(
        &mut self,
        _pid: Option<u32>,
        filter: &TreeFilter,
    ) -> impl Future<Output = Result<ElementTree>> {
        async move {
            // Clear previous cache
            self.cache.clear();
            self.element_bounds.clear();

            // Dump UI hierarchy
            let xml = self.adb.dump_ui()?;

            // Parse XML into node tree
            let root_node = parse_ui_xml(&xml)?;

            // Get package name if available
            let app_name = root_node.children.first().and_then(|n| n.package.clone());
            self.last_package = app_name.clone();

            // Convert to Element tree
            let root = root_node.to_element(&mut self.cache, &mut self.element_bounds);
            let element_count = self.cache.len();

            // Apply filter if needed
            let filtered_root =
                if filter.interactive_only || filter.visible_only || filter.max_depth.is_some() {
                    filter_element(root, filter, 0)
                } else {
                    root
                };

            Ok(ElementTree {
                version: self.cache.version(),
                pid: None, // Android doesn't use PID
                app_name,
                root: filtered_root,
                element_count,
            })
        }
    }

    fn get_element(&self, id: ElementKey) -> Option<&Element> {
        self.cache.get(id)
    }

    fn perform_action(
        &mut self,
        id: ElementKey,
        action: Action,
    ) -> impl Future<Output = Result<()>> {
        async move {
            match action {
                Action::Click => {
                    // Get element center and tap
                    let center = self
                        .get_element_center(id)
                        .ok_or_else(|| anyhow!("Element {} not found or has no bounds", id))?;
                    self.adb.tap(center.x, center.y)?;
                    Ok(())
                }
                Action::Focus => {
                    // Tap to focus
                    let center = self
                        .get_element_center(id)
                        .ok_or_else(|| anyhow!("Element {} not found or has no bounds", id))?;
                    self.adb.tap(center.x, center.y)?;
                    Ok(())
                }
                Action::ScrollIntoView => {
                    // Basic implementation: swipe up to scroll down
                    if let Some(size) = self.screen_size {
                        let center_x = size.0 as f64 / 2.0;
                        let start_y = size.1 as f64 * 0.7;
                        let end_y = size.1 as f64 * 0.3;
                        self.adb
                            .swipe((center_x, start_y), (center_x, end_y), 300)?;
                    }
                    Ok(())
                }
                _ => {
                    bail!("Action {:?} not supported on Android", action)
                }
            }
        }
    }

    fn set_value(&mut self, id: ElementKey, value: &str) -> impl Future<Output = Result<()>> {
        async move {
            // First tap to focus the element
            let center = self
                .get_element_center(id)
                .ok_or_else(|| anyhow!("Element {} not found or has no bounds", id))?;
            self.adb.tap(center.x, center.y)?;

            // Small delay to ensure focus
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

            // Clear existing text (select all and delete)
            self.adb.key_event(AndroidKeyCode::CtrlLeft as u32)?;
            self.adb.key_event(AndroidKeyCode::A as u32)?;
            self.adb.key_event(AndroidKeyCode::Del as u32)?;

            // Type the new value
            if !value.is_empty() {
                self.adb.input_text(value)?;
            }

            Ok(())
        }
    }

    fn hit_test(&mut self, x: f64, y: f64) -> impl Future<Output = Result<Option<ElementKey>>> {
        async move {
            let point = Point::new(x, y);

            // Search through cached elements for one containing the point
            // Return the deepest (most specific) element
            let mut best_match: Option<(ElementKey, f64)> = None; // (id, area)

            for (id, element) in self.cache.iter() {
                if let Some(bounds) = &element.bounds {
                    if bounds.contains(point) {
                        let area = bounds.size.width * bounds.size.height;
                        // Prefer smaller (more specific) elements
                        if best_match.is_none() || area < best_match.unwrap().1 {
                            best_match = Some((id, area));
                        }
                    }
                }
            }

            Ok(best_match.map(|(id, _)| id))
        }
    }

    fn clear_cache(&mut self) {
        self.cache.clear();
        self.element_bounds.clear();
    }

    fn snapshot_version(&self) -> u64 {
        self.cache.version()
    }

    fn capture_screen(&self, _pid: Option<u32>) -> Result<Screenshot> {
        let data = self.adb.screenshot()?;

        // Get image dimensions from PNG header
        let (width, height) = if data.len() > 24 {
            // PNG header: 8 bytes signature, then IHDR chunk
            // IHDR starts at byte 8, width at 16, height at 20 (both big-endian u32)
            let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
            let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
            (width, height)
        } else if let Some(size) = self.screen_size {
            size
        } else {
            (0, 0)
        };

        Ok(Screenshot {
            data,
            width,
            height,
        })
    }

    fn get_screen_bounds(&self, _pid: Option<u32>) -> impl Future<Output = Result<Rect>> {
        async move {
            let (width, height) = self.screen_size.ok_or_else(|| {
                anyhow!("Screen size not available. Call refresh_screen_size() first.")
            })?;
            Ok(Rect::new(
                Point::new(0.0, 0.0),
                Size::new(width as f64, height as f64),
            ))
        }
    }

    fn platform_name(&self) -> &'static str {
        "Android"
    }

    fn keystroke(
        &mut self,
        _pid: Option<u32>,
        key: Code,
        modifiers: Modifiers,
    ) -> impl Future<Output = Result<()>> {
        async move {
            // Press modifiers
            if modifiers.contains(Modifiers::SHIFT) {
                self.adb.key_event(AndroidKeyCode::ShiftLeft as u32)?;
            }
            if modifiers.contains(Modifiers::CONTROL) {
                self.adb.key_event(AndroidKeyCode::CtrlLeft as u32)?;
            }
            if modifiers.contains(Modifiers::ALT) {
                self.adb.key_event(AndroidKeyCode::AltLeft as u32)?;
            }
            if modifiers.contains(Modifiers::META) {
                self.adb.key_event(AndroidKeyCode::MetaLeft as u32)?;
            }

            // Press main key
            if let Some(keycode) = AndroidKeyCode::from_code(key) {
                self.adb.key_event(keycode as u32)?;
            } else {
                bail!("Unsupported key code: {:?}", key);
            }

            Ok(())
        }
    }

    fn type_raw(&mut self, _pid: Option<u32>, text: &str) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.input_text(text)?;
            Ok(())
        }
    }

    fn mouse_click_at(
        &mut self,
        _pid: Option<u32>,
        x: f64,
        y: f64,
        _button: MouseButton,
    ) -> impl Future<Output = Result<()>> {
        async move {
            // Android only supports single tap (no right-click)
            self.adb.tap(x, y)?;
            Ok(())
        }
    }

    fn mouse_scroll(
        &mut self,
        _pid: Option<u32>,
        delta_x: f64,
        delta_y: f64,
    ) -> impl Future<Output = Result<()>> {
        async move {
            let (width, height) = self
                .screen_size
                .ok_or_else(|| anyhow!("Screen size not available"))?;

            let center_x = width as f64 / 2.0;
            let center_y = height as f64 / 2.0;

            // Convert scroll deltas to swipe
            // Positive delta_y = scroll up, which is swipe down (start high, end low)
            let swipe_distance = 200.0; // pixels
            let start_x = center_x - delta_x * swipe_distance / 2.0;
            let start_y = center_y + delta_y * swipe_distance / 2.0;
            let end_x = center_x + delta_x * swipe_distance / 2.0;
            let end_y = center_y - delta_y * swipe_distance / 2.0;

            self.adb.swipe((start_x, start_y), (end_x, end_y), 100)?;
            Ok(())
        }
    }

    fn supports_keystroke(&self) -> bool {
        true
    }

    fn supports_mouse_click(&self) -> bool {
        true
    }

    fn supports_hit_test(&self) -> bool {
        true
    }

    fn supports_terminal_display(&self) -> bool {
        true
    }

    fn supports_event_listening(&self) -> bool {
        false // ADB doesn't support real-time event listening
    }

    fn supported_event_types(&self) -> Vec<AccessibilityEventType> {
        Vec::new() // No events supported via ADB
    }

    fn start_listening(
        &mut self,
        _config: ListenerConfig,
        _callback: Box<dyn FnMut(AccessibilityEvent) + Send + 'static>,
    ) -> Result<ListenerHandle> {
        bail!("Event listening is not supported on Android via ADB")
    }
}

/// Filter an element tree according to TreeFilter settings.
fn filter_element(elem: Element, filter: &TreeFilter, depth: usize) -> Element {
    let mut filtered = elem;

    // Filter children recursively
    filtered.children = filtered
        .children
        .into_iter()
        .filter(|child| filter.should_include(child, depth + 1))
        .map(|child| filter_element(child, filter, depth + 1))
        .collect();

    // Apply max_elements limit if set (this is a simple implementation)
    if let Some(max) = filter.max_elements {
        let total = count_elements(&filtered);
        if total > max {
            // Truncate children to fit
            filtered.children.truncate(max.saturating_sub(1));
        }
    }

    filtered
}

/// Count total elements in a subtree.
fn count_elements(elem: &Element) -> usize {
    1 + elem
        .children
        .iter()
        .map(|c| count_elements(c))
        .sum::<usize>()
}

// ============================================================================
// Android Extensions Trait
// ============================================================================

/// Android-specific extension methods.
///
/// These methods provide convenient access to Android-specific functionality
/// that isn't available on other platforms.
pub trait AndroidExtensions {
    /// Press the Android Back button.
    fn press_back(&mut self) -> impl Future<Output = Result<()>>;

    /// Press the Android Home button.
    fn press_home(&mut self) -> impl Future<Output = Result<()>>;

    /// Press the Recent Apps (App Switcher) button.
    fn press_recent_apps(&mut self) -> impl Future<Output = Result<()>>;

    /// Press the Android Menu button.
    fn press_menu(&mut self) -> impl Future<Output = Result<()>>;

    /// Increase volume.
    fn volume_up(&mut self) -> impl Future<Output = Result<()>>;

    /// Decrease volume.
    fn volume_down(&mut self) -> impl Future<Output = Result<()>>;

    /// Mute/unmute volume.
    fn volume_mute(&mut self) -> impl Future<Output = Result<()>>;

    /// Press the power button (screen on/off).
    fn press_power(&mut self) -> impl Future<Output = Result<()>>;

    /// Wake up the device.
    fn wake_up(&mut self) -> impl Future<Output = Result<()>>;

    /// Put the device to sleep.
    fn sleep(&mut self) -> impl Future<Output = Result<()>>;

    /// Launch an app by package name.
    ///
    /// # Arguments
    /// * `package` - The app's package name (e.g., "com.android.settings")
    fn launch_app(&mut self, package: &str) -> impl Future<Output = Result<()>>;

    /// Stop an app by package name.
    ///
    /// # Arguments
    /// * `package` - The app's package name
    fn stop_app(&mut self, package: &str) -> impl Future<Output = Result<()>>;

    /// Perform a swipe gesture.
    ///
    /// # Arguments
    /// * `start` - Starting coordinates (x, y) in screen pixels
    /// * `end` - Ending coordinates (x, y) in screen pixels
    /// * `duration_ms` - Duration of the swipe in milliseconds
    fn swipe(
        &mut self,
        start: (f64, f64),
        end: (f64, f64),
        duration_ms: u64,
    ) -> impl Future<Output = Result<()>>;

    /// Perform a long press at coordinates.
    ///
    /// # Arguments
    /// * `x` - X coordinate in screen pixels
    /// * `y` - Y coordinate in screen pixels
    /// * `duration_ms` - Duration of the long press in milliseconds
    fn long_press(&mut self, x: f64, y: f64, duration_ms: u64) -> impl Future<Output = Result<()>>;

    /// Get the current foreground activity info.
    fn get_current_activity(&self) -> impl Future<Output = Result<String>>;

    /// Open the notification shade.
    fn open_notifications(&mut self) -> impl Future<Output = Result<()>>;

    /// Open quick settings.
    fn open_quick_settings(&mut self) -> impl Future<Output = Result<()>>;
}

impl AndroidExtensions for AndroidAccessibility {
    fn press_back(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.key_event(AndroidKeyCode::Back as u32)?;
            Ok(())
        }
    }

    fn press_home(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.key_event(AndroidKeyCode::Home as u32)?;
            Ok(())
        }
    }

    fn press_recent_apps(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.key_event(AndroidKeyCode::AppSwitch as u32)?;
            Ok(())
        }
    }

    fn press_menu(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.key_event(AndroidKeyCode::Menu as u32)?;
            Ok(())
        }
    }

    fn volume_up(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.key_event(AndroidKeyCode::VolumeUp as u32)?;
            Ok(())
        }
    }

    fn volume_down(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.key_event(AndroidKeyCode::VolumeDown as u32)?;
            Ok(())
        }
    }

    fn volume_mute(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.key_event(AndroidKeyCode::VolumeMute as u32)?;
            Ok(())
        }
    }

    fn press_power(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.key_event(AndroidKeyCode::Power as u32)?;
            Ok(())
        }
    }

    fn wake_up(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.key_event(AndroidKeyCode::Wakeup as u32)?;
            Ok(())
        }
    }

    fn sleep(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.key_event(AndroidKeyCode::Sleep as u32)?;
            Ok(())
        }
    }

    fn launch_app(&mut self, package: &str) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.launch_app(package, None)?;
            Ok(())
        }
    }

    fn stop_app(&mut self, package: &str) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.stop_app(package)?;
            Ok(())
        }
    }

    fn swipe(
        &mut self,
        start: (f64, f64),
        end: (f64, f64),
        duration_ms: u64,
    ) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.swipe(start, end, duration_ms)?;
            Ok(())
        }
    }

    fn long_press(&mut self, x: f64, y: f64, duration_ms: u64) -> impl Future<Output = Result<()>> {
        async move {
            // Long press is a swipe with same start and end
            self.adb.swipe((x, y), (x, y), duration_ms)?;
            Ok(())
        }
    }

    fn get_current_activity(&self) -> impl Future<Output = Result<String>> {
        async move { self.adb.get_current_activity() }
    }

    fn open_notifications(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb
                .shell(&["cmd", "statusbar", "expand-notifications"])?;
            Ok(())
        }
    }

    fn open_quick_settings(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.shell(&["cmd", "statusbar", "expand-settings"])?;
            Ok(())
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bounds() {
        let bounds = parse_bounds("[0,0][1080,1920]").unwrap();
        assert_eq!(bounds.origin.x, 0.0);
        assert_eq!(bounds.origin.y, 0.0);
        assert_eq!(bounds.size.width, 1080.0);
        assert_eq!(bounds.size.height, 1920.0);

        let bounds = parse_bounds("[100,200][500,600]").unwrap();
        assert_eq!(bounds.origin.x, 100.0);
        assert_eq!(bounds.origin.y, 200.0);
        assert_eq!(bounds.size.width, 400.0);
        assert_eq!(bounds.size.height, 400.0);

        assert!(parse_bounds("invalid").is_none());
        assert!(parse_bounds("[0,0]").is_none());
        assert!(parse_bounds("").is_none());
    }

    #[test]
    fn test_role_mapping() {
        assert_eq!(
            map_android_class_to_role("android.widget.Button"),
            Role::Button
        );
        assert_eq!(
            map_android_class_to_role("android.widget.EditText"),
            Role::TextInput
        );
        assert_eq!(
            map_android_class_to_role("android.widget.TextView"),
            Role::Label
        );
        assert_eq!(
            map_android_class_to_role("android.widget.CheckBox"),
            Role::CheckBox
        );
        assert_eq!(
            map_android_class_to_role("android.widget.Switch"),
            Role::Switch
        );
        assert_eq!(
            map_android_class_to_role("android.widget.ListView"),
            Role::List
        );
        assert_eq!(
            map_android_class_to_role("android.widget.ScrollView"),
            Role::ScrollView
        );
        assert_eq!(
            map_android_class_to_role("android.widget.LinearLayout"),
            Role::GenericContainer
        );
        assert_eq!(
            map_android_class_to_role("android.widget.SeekBar"),
            Role::Slider
        );
        assert_eq!(
            map_android_class_to_role("android.widget.Spinner"),
            Role::ComboBox
        );
        assert_eq!(
            map_android_class_to_role("android.widget.ImageView"),
            Role::Image
        );
        assert_eq!(
            map_android_class_to_role("android.webkit.WebView"),
            Role::Document
        );
    }

    #[test]
    fn test_escape_shell_text() {
        assert_eq!(escape_shell_text("hello"), "hello");
        assert_eq!(escape_shell_text("hello world"), "hello%sworld");
        assert_eq!(escape_shell_text("test$var"), "test\\$var");
        assert_eq!(escape_shell_text("a&b"), "a\\&b");
    }

    #[test]
    fn test_android_keycode_mapping() {
        assert_eq!(
            AndroidKeyCode::from_code(Code::KeyA),
            Some(AndroidKeyCode::A)
        );
        assert_eq!(
            AndroidKeyCode::from_code(Code::Enter),
            Some(AndroidKeyCode::Enter)
        );
        assert_eq!(
            AndroidKeyCode::from_code(Code::Backspace),
            Some(AndroidKeyCode::Del)
        );
        assert_eq!(
            AndroidKeyCode::from_code(Code::ArrowUp),
            Some(AndroidKeyCode::DpadUp)
        );
        assert_eq!(
            AndroidKeyCode::from_code(Code::F1),
            Some(AndroidKeyCode::F1)
        );
    }

    #[test]
    fn test_parse_ui_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <hierarchy rotation="0">
          <node index="0" text="Hello" class="android.widget.TextView"
                bounds="[0,0][1080,100]" clickable="false" enabled="true" />
          <node index="1" text="" class="android.widget.Button"
                content-desc="Submit" bounds="[0,100][1080,200]"
                clickable="true" enabled="true">
            <node index="0" text="Submit" class="android.widget.TextView"
                  bounds="[10,110][1070,190]" clickable="false" enabled="true" />
          </node>
        </hierarchy>"#;

        let root = parse_ui_xml(xml).unwrap();
        assert_eq!(root.class, "hierarchy");
        assert_eq!(root.children.len(), 2);

        let text_view = &root.children[0];
        assert_eq!(text_view.class, "android.widget.TextView");
        assert_eq!(text_view.text.as_deref(), Some("Hello"));
        assert!(!text_view.clickable);

        let button = &root.children[1];
        assert_eq!(button.class, "android.widget.Button");
        assert_eq!(button.content_desc.as_deref(), Some("Submit"));
        assert!(button.clickable);
        assert_eq!(button.children.len(), 1);
    }
}
