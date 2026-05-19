use crate::parse::{PointArg, SwipeArg, parse_point, parse_swipe};
use accessibility_core::accessibility::TreeFilter;
use accessibility_core::api::OutputFormat;
use accessibility_core::input::MouseButton;
use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser, Clone, Debug)]
#[command(name = "accessibility-cli")]
#[command(version)]
#[command(arg_required_else_help = true)]
#[command(about = "Cross-platform accessibility tree inspection and automation CLI.")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Clone, Debug)]
pub enum Command {
    /// Print an accessibility tree.
    Tree(TreeCommand),
    /// Print elements matching a CSS-like selector.
    Query(QueryCommand),
    /// Click or activate an element by selector.
    Click(ElementCommand),
    /// Press an element by selector. Alias behavior for click.
    Press(ElementCommand),
    /// Focus an element by selector.
    Focus(ElementCommand),
    /// Blur an element by selector.
    Blur(ElementCommand),
    /// Set an element value.
    Type(TypeCommand),
    /// Focus an element and send a keystroke.
    Key(KeyCommand),
    /// Hit test a point.
    Hit(HitCommand),
    /// Click absolute desktop coordinates.
    MouseClick(MouseClickCommand),
    /// Tap device coordinates.
    Tap(TapCommand),
    /// Swipe device coordinates.
    Swipe(SwipeCommand),
    /// Long-press Android device coordinates.
    LongPress(LongPressCommand),
    /// Press a platform button.
    Button(ButtonCommand),
    /// Launch an Android application package.
    Launch(PlatformCommand),
    /// Stop an Android application package.
    Stop(PlatformCommand),
    /// Open Android notifications.
    Notifications(DeviceCommand),
    /// Open Android quick settings.
    QuickSettings(DeviceCommand),
    /// Wake an Android device.
    Wake(DeviceCommand),
    /// Put an Android device to sleep.
    Sleep(DeviceCommand),
    /// List windows and process IDs.
    ListWindows(ListWindowsCommand),
    /// Listen for accessibility events.
    Listen(ListenCommand),
    /// Capture screenshots.
    Screenshot(ScreenshotCommand),
    /// Test iOS Simulator framework loading.
    TestLoad(TestLoadCommand),
    /// Launch a macOS app off-screen-but-on-screen so its accessibility tree
    /// can be queried without interrupting the user. The window is shrunk to
    /// 1×1 in a corner and raised to a floating window level that most tiling
    /// window managers exclude from tiling rules.
    StealthLaunch(StealthLaunchCommand),
}

#[derive(Args, Clone, Debug)]
pub struct StealthLaunchCommand {
    /// App name (e.g. "Google Chrome") or path to .app bundle.
    pub app: String,
    /// Extra arguments to pass to the app. A leading URL is treated as the
    /// app's initial document.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
    /// Top-left X coordinate of the parked window.
    #[arg(long, default_value_t = 0)]
    pub x: i32,
    /// Top-left Y coordinate of the parked window.
    #[arg(long, default_value_t = 1)]
    pub y: i32,
    /// Window width. Default 1.
    #[arg(long, default_value_t = 1)]
    pub width: u32,
    /// Window height. Default 1.
    #[arg(long, default_value_t = 1)]
    pub height: u32,
    /// CGS window level. 3 = floating; tiling WMs exclude levels != 0.
    #[arg(long, default_value_t = 3)]
    pub level: i32,
    /// How long to wait for the app's first window after launch.
    #[arg(long, default_value_t = 5000, value_name = "MS")]
    pub window_timeout: u64,
    /// For Chromium-family apps, launch in `--app=URL` mode (frameless,
    /// no tab strip). The window has a non-standard subrole, which many
    /// tiling WMs (Amethyst, yabai, AeroSpace) exclude from tiling rules.
    /// If set, the first positional arg in `args` is taken as the URL.
    #[arg(long)]
    pub app_mode: bool,
}

#[derive(Args, Clone, Debug)]
pub struct TreeCommand {
    #[command(flatten)]
    pub target: TargetArgs,
    #[command(flatten)]
    pub filter: TreeFilterArgs,
    #[command(flatten)]
    pub output: OutputArgs,
}

#[derive(Args, Clone, Debug)]
pub struct QueryCommand {
    pub selector: String,
    #[command(flatten)]
    pub target: TargetArgs,
    #[command(flatten)]
    pub filter: TreeFilterArgs,
    #[command(flatten)]
    pub output: OutputArgs,
    #[command(flatten)]
    pub timeout: TimeoutArgs,
}

#[derive(Args, Clone, Debug)]
pub struct ElementCommand {
    pub selector: String,
    #[command(flatten)]
    pub target: TargetArgs,
    #[command(flatten)]
    pub filter: TreeFilterArgs,
    #[command(flatten)]
    pub timeout: TimeoutArgs,
}

#[derive(Args, Clone, Debug)]
pub struct TypeCommand {
    pub selector: String,
    pub text: String,
    #[command(flatten)]
    pub target: TargetArgs,
    #[command(flatten)]
    pub filter: TreeFilterArgs,
    #[command(flatten)]
    pub timeout: TimeoutArgs,
}

#[derive(Args, Clone, Debug)]
pub struct KeyCommand {
    pub key: String,
    pub selector: String,
    #[command(flatten)]
    pub target: TargetArgs,
    #[command(flatten)]
    pub filter: TreeFilterArgs,
    #[command(flatten)]
    pub timeout: TimeoutArgs,
}

#[derive(Args, Clone, Debug)]
pub struct HitCommand {
    #[arg(value_parser = parse_point)]
    pub point: PointArg,
    #[command(flatten)]
    pub target: TargetArgs,
}

#[derive(Args, Clone, Debug)]
pub struct MouseClickCommand {
    #[arg(value_parser = parse_point)]
    pub point: PointArg,
    #[arg(long, value_enum, default_value_t = MouseButtonArg::Left)]
    pub button: MouseButtonArg,
    #[command(flatten)]
    pub target: TargetArgs,
}

#[derive(Args, Clone, Debug)]
pub struct TapCommand {
    #[arg(value_parser = parse_point)]
    pub point: PointArg,
    #[arg(long, value_enum, default_value_t = InputMethodArg::Auto)]
    pub method: InputMethodArg,
    #[command(flatten)]
    pub target: TargetArgs,
}

#[derive(Args, Clone, Debug)]
pub struct SwipeCommand {
    #[arg(value_parser = parse_swipe)]
    pub points: SwipeArg,
    #[arg(long, default_value_t = 300, value_name = "MS")]
    pub duration: u64,
    #[arg(long, value_enum, default_value_t = InputMethodArg::Auto)]
    pub method: InputMethodArg,
    #[command(flatten)]
    pub target: TargetArgs,
}

#[derive(Args, Clone, Debug)]
pub struct LongPressCommand {
    #[arg(value_parser = parse_point)]
    pub point: PointArg,
    #[arg(long, default_value_t = 1000, value_name = "MS")]
    pub duration: u64,
    #[command(flatten)]
    pub target: TargetArgs,
}

#[derive(Args, Clone, Debug)]
pub struct ButtonCommand {
    #[arg(value_enum)]
    pub button: ButtonArg,
    #[command(flatten)]
    pub target: TargetArgs,
}

#[derive(Args, Clone, Debug)]
pub struct PlatformCommand {
    pub app_id: String,
    #[command(flatten)]
    pub target: TargetArgs,
}

#[derive(Args, Clone, Debug)]
pub struct DeviceCommand {
    #[command(flatten)]
    pub target: TargetArgs,
}

#[derive(Args, Clone, Debug)]
pub struct ListWindowsCommand {
    #[command(flatten)]
    pub target: TargetArgs,
    #[command(flatten)]
    pub output: OutputArgs,
}

#[derive(Args, Clone, Debug)]
pub struct ListenCommand {
    #[arg(long, value_delimiter = ',')]
    pub filter: Option<Vec<String>>,
    #[command(flatten)]
    pub target: TargetArgs,
}

#[derive(Args, Clone, Debug)]
pub struct ScreenshotCommand {
    #[command(subcommand)]
    pub command: ScreenshotSubcommand,
}

#[derive(Subcommand, Clone, Debug)]
pub enum ScreenshotSubcommand {
    /// Capture the full screen.
    Screen(ScreenshotScreenCommand),
    /// Capture matching or interactive elements.
    Elements(ScreenshotElementsCommand),
    /// Capture and annotate matching or interactive elements.
    Annotate(ScreenshotAnnotateCommand),
}

#[derive(Args, Clone, Debug)]
pub struct ScreenshotScreenCommand {
    #[command(flatten)]
    pub target: TargetArgs,
    #[command(flatten)]
    pub overlay: ScreenshotOverlayArgs,
}

#[derive(Args, Clone, Debug)]
pub struct ScreenshotElementsCommand {
    pub selector: Option<String>,
    #[command(flatten)]
    pub target: TargetArgs,
    #[command(flatten)]
    pub filter: TreeFilterArgs,
}

#[derive(Args, Clone, Debug)]
pub struct ScreenshotAnnotateCommand {
    pub selector: Option<String>,
    #[arg(long)]
    pub label: bool,
    #[command(flatten)]
    pub target: TargetArgs,
    #[command(flatten)]
    pub filter: TreeFilterArgs,
    #[command(flatten)]
    pub overlay: ScreenshotOverlayArgs,
}

#[derive(Args, Clone, Debug)]
pub struct TestLoadCommand {
    #[command(flatten)]
    pub target: TargetArgs,
}

#[derive(Args, Clone, Debug)]
pub struct TargetArgs {
    /// Target platform.
    #[arg(long, short = 'p', value_enum)]
    pub platform: PlatformType,
    /// Target application by process ID. Valid for mac, win, and linux.
    #[arg(long)]
    pub pid: Option<u32>,
    /// Target iOS Simulator by UDID. Defaults to the first booted simulator.
    #[arg(long)]
    pub udid: Option<String>,
    /// Target Android device by serial. Defaults to the only connected device.
    #[arg(long)]
    pub serial: Option<String>,
}

#[derive(Args, Clone, Debug, Default)]
pub struct TreeFilterArgs {
    #[arg(long)]
    pub depth: Option<usize>,
    #[arg(long)]
    pub interactive: bool,
    #[arg(long)]
    pub visible: bool,
}

impl TreeFilterArgs {
    pub fn to_tree_filter(&self) -> TreeFilter {
        TreeFilter {
            max_depth: self.depth,
            max_elements: None,
            interactive_only: self.interactive,
            visible_only: self.visible,
            within_bounds: None,
            roles: None,
        }
    }
}

#[derive(Args, Clone, Debug)]
pub struct OutputArgs {
    #[arg(long, value_enum, default_value_t = OutputFormatArg::Llm)]
    pub format: OutputFormatArg,
    #[arg(long)]
    pub structure: bool,
}

impl OutputArgs {
    pub fn output_format(&self) -> OutputFormat {
        self.format.into()
    }
}

#[derive(Args, Clone, Copy, Debug)]
pub struct TimeoutArgs {
    #[arg(long, default_value_t = 30000, value_name = "MS")]
    pub timeout: u64,
    #[arg(long, default_value_t = 100, value_name = "MS")]
    pub poll_interval: u64,
}

impl TimeoutArgs {
    pub fn should_poll(self) -> bool {
        self.timeout > 0
    }
}

#[derive(Args, Clone, Copy, Debug)]
pub struct ScreenshotOverlayArgs {
    #[arg(long)]
    pub overlay: bool,
    #[arg(long, default_value_t = 100)]
    pub grid_size: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum PlatformType {
    #[value(name = "mac")]
    MacOS,
    #[value(name = "win")]
    Windows,
    #[value(name = "ios")]
    IOS,
    #[value(name = "linux")]
    Linux,
    #[value(name = "android")]
    Android,
}

impl PlatformType {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::MacOS => "macOS",
            Self::Windows => "Windows",
            Self::IOS => "iOS",
            Self::Linux => "Linux",
            Self::Android => "Android",
        }
    }

    pub fn is_pid_target(self) -> bool {
        matches!(self, Self::MacOS | Self::Windows | Self::Linux)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum OutputFormatArg {
    Tree,
    Json,
    Llm,
}

impl From<OutputFormatArg> for OutputFormat {
    fn from(value: OutputFormatArg) -> Self {
        match value {
            OutputFormatArg::Tree => OutputFormat::Tree,
            OutputFormatArg::Json => OutputFormat::Json,
            OutputFormatArg::Llm => OutputFormat::LlmQuery,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum InputMethodArg {
    Auto,
    Accessibility,
    Hid,
    Adb,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum MouseButtonArg {
    Left,
    Right,
    Middle,
}

impl From<MouseButtonArg> for MouseButton {
    fn from(value: MouseButtonArg) -> Self {
        match value {
            MouseButtonArg::Left => MouseButton::Left,
            MouseButtonArg::Right => MouseButton::Right,
            MouseButtonArg::Middle => MouseButton::Middle,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ButtonArg {
    Back,
    Home,
    Recent,
    Menu,
    VolumeUp,
    VolumeDown,
    Lock,
    Siri,
    Side,
}
