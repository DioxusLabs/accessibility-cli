//! Cross-platform accessibility CLI for macOS, Windows, iOS Simulator, Linux, and Android.

mod cli;
mod error;
mod operations;
mod parse;
mod runner;
mod target;

use clap::Parser;

pub use cli::{
    ButtonArg, Cli, Command, ElementCommand, HitCommand, InputMethodArg, KeyCommand,
    ListWindowsCommand, LongPressCommand, MouseButtonArg, MouseClickCommand, OutputArgs,
    OutputFormatArg, PlatformCommand, PlatformType, QueryCommand, ScreenshotAnnotateCommand,
    ScreenshotCommand, ScreenshotElementsCommand, ScreenshotOverlayArgs, ScreenshotScreenCommand,
    ScreenshotSubcommand, SwipeCommand, TargetArgs, TestLoadCommand, TimeoutArgs, TreeCommand,
    TreeFilterArgs, TypeCommand,
};
pub use error::{CliError, CliResult};
pub use parse::{MouseClickParams, PointArg, SwipeArg, SwipeParams};

/// Run the CLI using process arguments.
pub fn run() {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Failed to create tokio runtime: {}", e);
            std::process::exit(1);
        }
    };

    if let Err(error) = runtime.block_on(run_cli(cli)) {
        match &error {
            CliError::Usage(message) => eprintln!("error: {}", message),
            CliError::Runtime(message) => eprintln!("{}", message),
        }
        std::process::exit(error.exit_code());
    }
}

/// Run a parsed CLI command.
pub async fn run_cli(cli: Cli) -> CliResult<()> {
    runner::run_cli(cli).await
}
