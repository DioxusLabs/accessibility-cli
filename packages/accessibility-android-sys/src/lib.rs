//! Low-level ADB wrappers used by accessibility-cli's Android backend.

pub mod emulator;

use std::process::{Output, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use keyboard_types::Code;
use tokio::process::Command;

const UI_DUMP_ATTEMPTS: usize = 3;
const UI_DUMP_RETRY_DELAY: Duration = Duration::from_millis(500);
pub const DEFAULT_ADB_TIMEOUT: Duration = Duration::from_secs(30);

/// Android key codes for `input keyevent` command.
///
/// These correspond to the KEYCODE_* constants in Android's KeyEvent class.
/// See: <https://developer.android.com/reference/android/view/KeyEvent>
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
    Del = 67,
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
    AppSwitch = 187,
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
            Code::ArrowUp => AndroidKeyCode::DpadUp,
            Code::ArrowDown => AndroidKeyCode::DpadDown,
            Code::ArrowLeft => AndroidKeyCode::DpadLeft,
            Code::ArrowRight => AndroidKeyCode::DpadRight,
            Code::Home => AndroidKeyCode::MoveHome,
            Code::End => AndroidKeyCode::MoveEnd,
            Code::PageUp => AndroidKeyCode::PageUp,
            Code::PageDown => AndroidKeyCode::PageDown,
            Code::Enter => AndroidKeyCode::Enter,
            Code::NumpadEnter => AndroidKeyCode::NumpadEnter,
            Code::Backspace => AndroidKeyCode::Del,
            Code::Delete => AndroidKeyCode::ForwardDel,
            Code::Insert => AndroidKeyCode::Insert,
            Code::Tab => AndroidKeyCode::Tab,
            Code::Escape => AndroidKeyCode::Escape,
            Code::Space => AndroidKeyCode::Space,
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

/// ADB command execution wrapper.
///
/// Provides methods to execute ADB commands with optional device targeting.
#[derive(Debug, Clone)]
pub struct AdbClient {
    /// Device serial number for multi-device scenarios (from `adb devices`).
    pub serial: Option<String>,
    /// Path to the ADB binary.
    pub adb_path: String,
    /// Maximum time to wait for an ADB command.
    pub timeout: Duration,
}

impl Default for AdbClient {
    fn default() -> Self {
        Self {
            serial: None,
            adb_path: "adb".to_string(),
            timeout: DEFAULT_ADB_TIMEOUT,
        }
    }
}

impl AdbClient {
    /// Create a new ADB client.
    pub fn new(serial: Option<&str>) -> Self {
        Self {
            serial: serial.map(String::from),
            adb_path: "adb".to_string(),
            timeout: DEFAULT_ADB_TIMEOUT,
        }
    }

    pub fn discover(serial: Option<&str>) -> Self {
        for root in [
            std::env::var_os("ANDROID_SDK_ROOT"),
            std::env::var_os("ANDROID_HOME"),
        ]
        .into_iter()
        .flatten()
        {
            let path = std::path::PathBuf::from(root)
                .join("platform-tools")
                .join(adb_binary_name());
            if path.is_file() {
                return Self::with_adb_path(serial, &path.to_string_lossy());
            }
        }
        if let Some(home) = std::env::var_os("HOME") {
            let home = std::path::PathBuf::from(home);
            let roots = if cfg!(target_os = "macos") {
                vec![home.join("Library/Android/sdk"), home.join("Android/Sdk")]
            } else {
                vec![home.join("Android/Sdk")]
            };
            for root in roots {
                let path = root.join("platform-tools").join(adb_binary_name());
                if path.is_file() {
                    return Self::with_adb_path(serial, &path.to_string_lossy());
                }
            }
        }
        Self::new(serial)
    }

    /// Create a new ADB client with a custom ADB path.
    pub fn with_adb_path(serial: Option<&str>, adb_path: &str) -> Self {
        Self {
            serial: serial.map(String::from),
            adb_path: adb_path.to_string(),
            timeout: DEFAULT_ADB_TIMEOUT,
        }
    }

    /// Set the maximum time to wait for an ADB command.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Build base ADB command with optional device serial.
    fn base_command(&self) -> Command {
        let mut cmd = Command::new(&self.adb_path);
        if let Some(ref serial) = self.serial {
            cmd.arg("-s").arg(serial);
        }
        cmd
    }

    async fn run(&self, kind: &str, args: &[&str], leading: Option<&str>) -> Result<Output> {
        let mut cmd = self.base_command();
        if let Some(leading) = leading {
            cmd.arg(leading);
        }
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let child = cmd.spawn().with_context(|| {
            if kind == "devices" {
                format!(
                    "ADB binary not found at '{}'. Install Android SDK Platform Tools.",
                    self.adb_path
                )
            } else {
                format!("Failed to execute adb {kind} command")
            }
        })?;
        tokio::time::timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "ADB binary '{}' {kind} command timed out after {:?}",
                    self.adb_path,
                    self.timeout
                )
            })?
            .with_context(|| format!("Failed to execute adb {kind} command"))
    }

    /// Execute an ADB shell command.
    pub async fn shell(&self, args: &[&str]) -> Result<String> {
        let output = self.run("shell", args, Some("shell")).await?;
        Self::check_output(&output, "shell")?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Execute an ADB shell command and return raw bytes.
    pub async fn shell_raw(&self, args: &[&str]) -> Result<Vec<u8>> {
        let output = self.run("shell", args, Some("shell")).await?;
        Self::check_output(&output, "shell")?;
        Ok(output.stdout)
    }

    /// Execute `adb exec-out` for efficient binary output.
    pub async fn exec_out(&self, args: &[&str]) -> Result<Vec<u8>> {
        let output = self.run("exec-out", args, Some("exec-out")).await?;
        Self::check_output(&output, "exec-out")?;
        Ok(output.stdout)
    }

    /// Execute a general ADB command (not shell).
    pub async fn command(&self, args: &[&str]) -> Result<String> {
        let output = self.run("adb", args, None).await?;
        Self::check_output(&output, "adb")?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

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
    pub async fn check_connection(&self) -> Result<()> {
        let devices = self.connected_devices().await?;
        if devices.is_empty() {
            bail!("No Android devices connected. Connect a device or start an emulator.");
        }
        if let Some(serial) = &self.serial
            && !devices.contains(serial)
        {
            bail!(
                "Device '{}' is not connected. Available devices: {}",
                serial,
                devices.join(", ")
            );
        }
        Ok(())
    }

    pub async fn connected_devices(&self) -> Result<Vec<String>> {
        let output = self.run("devices", &[], Some("devices")).await?;
        Self::check_output(&output, "devices")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout
            .lines()
            .skip(1)
            .filter_map(|line| {
                let (serial, state) = line.split_once('\t')?;
                (state.split_whitespace().next() == Some("device")).then(|| serial.to_string())
            })
            .collect())
    }

    pub async fn resolved_serial(&self) -> Result<String> {
        let devices = self.connected_devices().await?;
        if let Some(serial) = &self.serial {
            if devices.contains(serial) {
                return Ok(serial.clone());
            }
            bail!(
                "Device '{}' is not connected. Available devices: {}",
                serial,
                devices.join(", ")
            );
        }
        match devices.as_slice() {
            [serial] => Ok(serial.clone()),
            [] => bail!("No Android devices connected. Connect a device or start an emulator."),
            _ => bail!(
                "Multiple Android devices are connected; specify one of: {}",
                devices.join(", ")
            ),
        }
    }

    /// Get the screen size in pixels.
    pub async fn get_screen_size(&self) -> Result<(u32, u32)> {
        let output = self.shell(&["wm", "size"]).await?;
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
    pub async fn screenshot(&self) -> Result<Vec<u8>> {
        self.exec_out(&["screencap", "-p"]).await
    }

    /// Tap at screen coordinates.
    pub async fn tap(&self, x: f64, y: f64) -> Result<()> {
        self.shell(&[
            "input",
            "tap",
            &x.round().to_string(),
            &y.round().to_string(),
        ])
        .await?;
        Ok(())
    }

    /// Swipe from one point to another.
    pub async fn swipe(&self, start: (f64, f64), end: (f64, f64), duration_ms: u64) -> Result<()> {
        self.shell(&[
            "input",
            "swipe",
            &start.0.round().to_string(),
            &start.1.round().to_string(),
            &end.0.round().to_string(),
            &end.1.round().to_string(),
            &duration_ms.to_string(),
        ])
        .await?;
        Ok(())
    }

    /// Send a key event.
    pub async fn key_event(&self, keycode: u32) -> Result<()> {
        self.shell(&["input", "keyevent", &keycode.to_string()])
            .await?;
        Ok(())
    }

    /// Send text input.
    pub async fn input_text(&self, text: &str) -> Result<()> {
        let escaped = escape_shell_text(text);
        self.shell(&["input", "text", &escaped]).await?;
        Ok(())
    }

    /// Dump the UI hierarchy as XML.
    pub async fn dump_ui(&self) -> Result<String> {
        let mut last_error = None;

        for attempt in 1..=UI_DUMP_ATTEMPTS {
            match self.dump_ui_once().await {
                Ok(xml) => return Ok(xml),
                Err(error) => {
                    last_error = Some(error);
                    if attempt < UI_DUMP_ATTEMPTS {
                        tokio::time::sleep(UI_DUMP_RETRY_DELAY).await;
                    }
                }
            }
        }

        Err(last_error.expect("UI dump should be attempted")).context(format!(
            "Failed to dump Android UI after {UI_DUMP_ATTEMPTS} attempts"
        ))
    }

    async fn dump_ui_once(&self) -> Result<String> {
        let result = self.shell(&["uiautomator", "dump", "/dev/tty"]).await;

        match result {
            Ok(output) => match extract_ui_xml(&output) {
                Some(xml) => Ok(xml),
                None => self.dump_ui_via_file().await.with_context(|| {
                    format!(
                        "direct uiautomator dump did not contain XML: {}",
                        truncate_for_error(&output)
                    )
                }),
            },
            Err(error) => self
                .dump_ui_via_file()
                .await
                .with_context(|| format!("direct uiautomator dump failed: {error}")),
        }
    }

    async fn dump_ui_via_file(&self) -> Result<String> {
        let tmp_path = "/data/local/tmp/window_dump.xml";

        let _ = self.shell(&["rm", "-f", tmp_path]).await;
        let dump_output = self.shell(&["uiautomator", "dump", tmp_path]).await?;
        if let Some(xml) = extract_ui_xml(&dump_output) {
            let _ = self.shell(&["rm", "-f", tmp_path]).await;
            return Ok(xml);
        }

        let xml = self.shell(&["cat", tmp_path]).await.with_context(|| {
            format!(
                "uiautomator dump did not create readable file at {tmp_path}; dump output: {}",
                truncate_for_error(&dump_output)
            )
        })?;
        let _ = self.shell(&["rm", "-f", tmp_path]).await;

        if let Some(xml) = extract_ui_xml(&xml) {
            Ok(xml)
        } else {
            bail!("Failed to parse UI dump XML: {}", truncate_for_error(&xml));
        }
    }

    /// Launch an app by package name and optional activity.
    pub async fn launch_app(&self, package: &str, activity: Option<&str>) -> Result<()> {
        match activity {
            Some(act) => {
                let component = format!("{}/{}", package, act);
                self.shell(&["am", "start", "-n", &component]).await?;
            }
            None => {
                self.shell(&[
                    "monkey",
                    "-p",
                    package,
                    "-c",
                    "android.intent.category.LAUNCHER",
                    "1",
                ])
                .await?;
            }
        }
        Ok(())
    }

    /// Force stop an app.
    pub async fn stop_app(&self, package: &str) -> Result<()> {
        self.shell(&["am", "force-stop", package]).await?;
        Ok(())
    }

    /// Get the current foreground activity.
    pub async fn get_current_activity(&self) -> Result<String> {
        let output = self.shell(&["dumpsys", "activity", "activities"]).await?;

        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("mResumedActivity:") || trimmed.starts_with("mFocusedActivity:")
            {
                return Ok(trimmed.to_string());
            }
        }

        let output = self.shell(&["dumpsys", "window", "windows"]).await?;
        for line in output.lines() {
            if line.contains("mCurrentFocus") || line.contains("mFocusedApp") {
                return Ok(line.trim().to_string());
            }
        }

        bail!("Could not determine current activity");
    }
}

fn adb_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "adb.exe"
    } else {
        "adb"
    }
}

/// Escape text for ADB shell input command.
pub fn escape_shell_text(text: &str) -> String {
    let mut result = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        match c {
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

fn extract_ui_xml(output: &str) -> Option<String> {
    output
        .find("<?xml")
        .or_else(|| output.find("<hierarchy"))
        .map(|start| output[start..].to_string())
}

fn truncate_for_error(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return "<empty>".to_string();
    }

    let mut chars = trimmed.chars();
    let truncated = chars.by_ref().take(200).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_shell_text() {
        assert_eq!(escape_shell_text("hello"), "hello");
        assert_eq!(escape_shell_text("hello world"), "hello%sworld");
        assert_eq!(escape_shell_text("test$var"), "test\\$var");
        assert_eq!(escape_shell_text("a&b"), "a\\&b");
    }

    #[test]
    fn test_extract_ui_xml() {
        assert_eq!(
            extract_ui_xml("UI dump\n<?xml version=\"1.0\" ?><hierarchy />"),
            Some("<?xml version=\"1.0\" ?><hierarchy />".to_string())
        );
        assert_eq!(
            extract_ui_xml("Noise <hierarchy rotation=\"0\" />"),
            Some("<hierarchy rotation=\"0\" />".to_string())
        );
        assert_eq!(extract_ui_xml("UI hierchary dumped to file"), None);
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

    #[cfg(unix)]
    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn adb_command_times_out_and_kills_child() {
        let adb =
            AdbClient::with_adb_path(None, "/bin/sleep").with_timeout(Duration::from_millis(100));
        let started = std::time::Instant::now();
        let error = adb.command(&["5"]).await.unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(error.to_string().contains("timed out after"));
    }

    #[cfg(unix)]
    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    async fn missing_adb_binary_has_install_context() {
        let adb = AdbClient::with_adb_path(None, "/no/such/adb");
        let error = adb.connected_devices().await.unwrap_err();
        assert!(error.to_string().contains(
            "ADB binary not found at '/no/such/adb'. Install Android SDK Platform Tools."
        ));
    }
}
