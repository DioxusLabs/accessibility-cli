//! Stealth-launch: spawn a macOS app in the background and park its window
//! at a 1×1 floating tile so its accessibility tree can be queried without
//! taking focus, without occupying screen real estate, and without being
//! grabbed by a tiling window manager.
//!
//! Why this works:
//! - We spawn the app's binary directly, so the PID is in hand immediately
//!   (no race against `NSWorkspace`'s `runningApplications` snapshot).
//! - For Chromium-family apps we also prepend `--window-size=W,H` and
//!   `--window-position=X,Y`, so the very first frame the WM sees is
//!   already tiny. The window is then bumped to a floating CGS level so
//!   tiling window managers (yabai, AeroSpace, Amethyst, Rectangle) exclude
//!   it from their rules. Order: level → size → position.
//! - For non-Chromium apps we fall back to post-launch AX resize.
//!
//! `WebContents::GetVisibility() != HIDDEN` requires at least one on-screen
//! pixel, so we cannot launch fully hidden. `1×1` at the screen corner is
//! enough to satisfy the visibility gate while staying out of the user's way.

use crate::cli::StealthLaunchCommand;
use crate::error::{CliError, CliResult};

#[cfg(not(target_os = "macos"))]
pub async fn stealth_launch(_command: &StealthLaunchCommand) -> CliResult<()> {
    Err(CliError::usage("stealth-launch is supported only on macOS"))
}

#[cfg(target_os = "macos")]
pub async fn stealth_launch(command: &StealthLaunchCommand) -> CliResult<()> {
    use accessibility_macos_sys::{AxElement, Point, Size, set_window_level};
    use std::time::{Duration, Instant};
    use tokio::process::Command as TokioCommand;

    const POLL_INTERVAL: Duration = Duration::from_millis(50);

    let executable = resolve_executable(&command.app)?;
    let chromium_like = is_chromium_executable(&executable);
    let window_timeout = Duration::from_millis(command.window_timeout);

    let mut launch_args: Vec<String> = Vec::new();
    let mut passthrough = command.args.clone();
    if chromium_like {
        if command.app_mode {
            // Pull the leading URL out and pass it via --app= so Chrome
            // launches a frameless app-style window. Amethyst's tiling rules
            // skip the resulting non-standard window subrole.
            let url = passthrough
                .iter()
                .position(|arg| !arg.starts_with("--"))
                .map(|idx| passthrough.remove(idx));
            if let Some(url) = url {
                launch_args.push(format!("--app={url}"));
            } else {
                return Err(CliError::usage(
                    "`--app-mode` requires a URL as the first positional arg",
                ));
            }
        }
        launch_args.push(format!("--window-size={},{}", command.width, command.height));
        launch_args.push(format!("--window-position={},{}", command.x, command.y));
        launch_args.push("--no-first-run".to_string());
    }
    launch_args.extend(passthrough);

    let mut spawn = TokioCommand::new(&executable);
    spawn.args(&launch_args);
    // Detach: we don't want the launched app's lifetime tied to ours.
    spawn.stdin(std::process::Stdio::null());
    spawn.stdout(std::process::Stdio::null());
    spawn.stderr(std::process::Stdio::null());
    let child = spawn.spawn().map_err(|err| {
        CliError::runtime(format!(
            "failed to spawn `{}`: {err}",
            executable.display(),
        ))
    })?;
    let pid = child
        .id()
        .ok_or_else(|| CliError::runtime("spawned child has no PID"))?;

    let app_element = AxElement::application(pid);
    let deadline = Instant::now() + window_timeout;
    let window = loop {
        let windows = app_element.attribute_elements("AXWindows");
        if let Some(window) = windows.into_iter().next() {
            break window;
        }
        if Instant::now() >= deadline {
            return Err(CliError::runtime(format!(
                "`{}` (pid {pid}) launched but no window appeared within {} ms",
                command.app, command.window_timeout,
            )));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    };

    let window_id = window.window_id().ok_or_else(|| {
        CliError::runtime(format!(
            "could not resolve window id for `{}` (pid {pid})",
            command.app,
        ))
    })?;

    // Bump the window level *first* so any tiling WM that's watching the
    // window-server's notification stream marks this window as floating
    // before it tries to apply tiling rules to a normal-level window.
    if !set_window_level(window_id, command.level) {
        eprintln!(
            "warning: SLSSetWindowLevel({}, {}) was rejected; tiling-WM evasion may not work",
            window_id.0, command.level,
        );
    }

    // Re-apply size/position a few times. Some tiling WMs see the window
    // first and resize/move it before we get a chance; re-enforcing for a
    // short interval beats the race in practice.
    let enforce_deadline = Instant::now() + Duration::from_millis(750);
    loop {
        let _ = window.set_size_attribute(
            "AXSize",
            Size::new(command.width as f64, command.height as f64),
        );
        let _ = window.set_point_attribute(
            "AXPosition",
            Point::new(command.x as f64, command.y as f64),
        );
        if Instant::now() >= enforce_deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    println!("{pid}");
    // Don't reap: we want the launched app to outlive us.
    drop(child);
    Ok(())
}

#[cfg(target_os = "macos")]
fn resolve_executable(app: &str) -> CliResult<std::path::PathBuf> {
    use std::path::{Path, PathBuf};

    // 1. Direct binary path.
    let direct = PathBuf::from(app);
    if direct.is_file() {
        return Ok(direct);
    }
    // 2. ".app" bundle path → Contents/MacOS/<exec>.
    if direct.extension().is_some_and(|e| e == "app") && direct.is_dir() {
        if let Some(exec) = bundle_executable(&direct) {
            return Ok(exec);
        }
    }
    // 3. Bare name → look under /Applications.
    let bare_bundle = Path::new("/Applications").join(format!("{app}.app"));
    if bare_bundle.is_dir() {
        if let Some(exec) = bundle_executable(&bare_bundle) {
            return Ok(exec);
        }
    }
    Err(CliError::usage(format!(
        "could not resolve `{app}` to an executable (tried direct path, .app bundle, /Applications)",
    )))
}

#[cfg(target_os = "macos")]
fn bundle_executable(bundle: &std::path::Path) -> Option<std::path::PathBuf> {
    let macos_dir = bundle.join("Contents/MacOS");
    let entries = std::fs::read_dir(&macos_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn is_chromium_executable(path: &std::path::Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    // .../<App>.app/Contents/MacOS/<exe>; bundle root is parent.parent.
    let bundle = parent.parent().and_then(|p| p.parent());
    let Some(bundle) = bundle else { return false };
    let frameworks = bundle.join("Contents/Frameworks");
    if !frameworks.exists() {
        return false;
    }
    std::fs::read_dir(&frameworks)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains("Chromium Framework")
                || entry
                    .file_name()
                    .to_string_lossy()
                    .contains("Chrome Framework")
        })
}
