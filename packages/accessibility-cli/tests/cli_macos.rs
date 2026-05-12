//! macOS CLI integration tests.
//!
//! These exercise the CLI binary against a real backgrounded Calculator, which
//! is what library-only tests historically missed: every previous test ended up
//! with Calculator frontmost, so the bugs around AXChildren omitting windows for
//! non-frontmost apps never surfaced.

#![cfg(target_os = "macos")]

use assert_cmd::Command as TestCommand;
use predicates::prelude::*;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Launch Calculator in the background and return its PID.
///
/// We never activate Calculator — the whole point of this library is to work
/// against backgrounded apps without disturbing the user's frontmost window.
/// If Calculator is already running but has zero windows (it can sit in this
/// dormant state after the user closes the window), we quit and relaunch it
/// so the test gets a fresh window without ever calling `activate`.
fn launch_calculator_backgrounded() -> u32 {
    // If Calculator is alive but has no AX-visible window, quit it so the
    // relaunch below produces a fresh window. `open -g -a` on an already-
    // running process doesn't create new windows.
    if let Some(p) = calculator_pid()
        && calculator_appears_windowless(p)
    {
        let _ = Command::new("osascript")
            .args(["-e", "tell application \"Calculator\" to quit"])
            .status();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && calculator_pid().is_some() {
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    // Start Calculator without bringing it to the front.
    let status = Command::new("open")
        .args(["-g", "-a", "Calculator"])
        .status()
        .expect("Failed to launch Calculator");
    assert!(status.success(), "open -g -a Calculator failed");

    // Wait for the process to register.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut pid = None;
    while Instant::now() < deadline {
        if let Some(p) = calculator_pid() {
            pid = Some(p);
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let pid = pid.expect("Timed out waiting for Calculator to launch");

    // Wait for Calculator to materialize its window — `open -g` returns
    // before the AX tree is fully usable.
    ensure_calculator_has_window();

    // If Calculator happens to already be frontmost (the user had it open),
    // the test still runs but the backgrounded-specific assertion path is
    // trivially satisfied. We never steal focus to make it pass.
    if frontmost_app().as_deref() == Some("Calculator") {
        eprintln!(
            "warning: Calculator is currently frontmost; \
             backgrounded-tree assertion will be trivially satisfied. \
             Re-run with another app focused for full coverage."
        );
    }

    pid
}

/// Check whether Calculator currently exposes a button — via the CLI under
/// test. This is the right readiness signal because it uses the same AX
/// query path the tests will hit, and it works for backgrounded apps where
/// `System Events` may report zero windows.
fn calculator_has_buttons(pid: u32) -> bool {
    let out = Command::new(env!("CARGO_BIN_EXE_accessibility-cli"))
        .args([
            "--platform",
            "mac",
            "--pid",
            &pid.to_string(),
            "--query",
            "Button",
            "--timeout",
            "0",
        ])
        .output();
    let Ok(out) = out else { return false };
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout.contains("Found ") && stdout.contains("match")
}

/// Calc process exists with no AX window (the user closed it without
/// quitting). Detect this via the public AX surface — we query the tree
/// once and check whether a Window-role element appears. We can't rely on
/// `System Events` to count windows because under our AXWindows + AXMainWindow
/// fix the AX tree may surface a window that `System Events` doesn't see.
fn calculator_appears_windowless(pid: u32) -> bool {
    let out = Command::new(env!("CARGO_BIN_EXE_accessibility-cli"))
        .args([
            "--platform",
            "mac",
            "--pid",
            &pid.to_string(),
            "--query",
            "Window",
            "--timeout",
            "0",
        ])
        .output();
    let Ok(out) = out else { return true };
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout.contains("No matches found")
}

fn ensure_calculator_has_window() {
    // Calculator can take several seconds after `open -g -a` to materialize
    // its window — especially when relaunching after a prior quit. Just wait;
    // we don't try to force the window open since Calculator doesn't accept
    // `make new document` and any other AppleScript trick steals focus.
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Some(pid) = calculator_pid()
            && calculator_has_buttons(pid)
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("Calculator process is running but never opened a window within 15s");
}

fn calculator_pid() -> Option<u32> {
    let script = r#"
        try
            tell application "System Events"
                unix id of first process whose name is "Calculator"
            end tell
        on error
            return ""
        end try
    "#;
    let output = Command::new("osascript")
        .args(["-e", script])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

fn frontmost_app() -> Option<String> {
    let output = Command::new("osascript")
        .args([
            "-e",
            "tell application \"System Events\" to name of first application process whose frontmost is true",
        ])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// End-to-end backgrounded math: launch Calculator backgrounded, drive
/// 1001992 + 299188 = 1301180 via `--click`, then verify the display reads
/// 1,301,180. This is the user-visible promise of the whole library — the
/// click chain has to *actually compute the right answer* in a non-frontmost
/// app — so we test it directly.
#[test]
#[serial_test::file_serial(calculator)]
fn backgrounded_calculator_computes_real_math() {
    let pid = launch_calculator_backgrounded();
    reset_calculator_display(pid);

    // Click each digit, the operator, more digits, then Equals.
    let sequence: &[&str] = &[
        "1", "0", "0", "1", "9", "9", "2", "Add", "2", "9", "9", "1", "8", "8", "Equals",
    ];
    for desc in sequence {
        TestCommand::cargo_bin("accessibility-cli")
            .unwrap()
            .args([
                "--platform",
                "mac",
                "--pid",
                &pid.to_string(),
                "--click",
                &format!("Button[description=\"{desc}\"]"),
                "--timeout",
                "5000",
            ])
            .assert()
            .success();
    }

    // Verify the Calculator's display shows the comma-formatted result.
    let assert = TestCommand::cargo_bin("accessibility-cli")
        .unwrap()
        .args([
            "--platform",
            "mac",
            "--pid",
            &pid.to_string(),
            "--query",
            "Text",
            "--timeout",
            "5000",
        ])
        .assert()
        .success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        out.contains("1,301,180"),
        "expected Calculator to compute 1001992+299188=1,301,180 in the background; got:\n{out}"
    );
}

/// Bug 1: tree of a backgrounded macOS app must include its Window, not just
/// the menu bar. Before the fix the LLM dump only contained MenuItem rows.
#[test]
#[serial_test::file_serial(calculator)]
fn backgrounded_app_tree_includes_window_buttons() {
    let pid = launch_calculator_backgrounded();

    let assert = TestCommand::cargo_bin("accessibility-cli")
        .unwrap()
        .args([
            "--platform",
            "mac",
            "--pid",
            &pid.to_string(),
            "--llm",
            // Don't poll forever if something regresses; query mode is what we
            // want to actually surface buttons in concise output.
        ])
        .assert()
        .success();

    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        out.contains("Button \"5\""),
        "expected Calculator's '5' button in --llm output; got:\n{out}"
    );
}

/// Bug 2: --interactive must produce a tree, not "Failed to build
/// accessibility tree".
#[test]
#[serial_test::file_serial(calculator)]
fn interactive_filter_returns_tree_not_error() {
    let pid = launch_calculator_backgrounded();

    TestCommand::cargo_bin("accessibility-cli")
        .unwrap()
        .args([
            "--platform",
            "mac",
            "--pid",
            &pid.to_string(),
            "--interactive",
            "--llm",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("Failed to build accessibility tree").not())
        .stdout(predicate::str::contains("Calculator"));
}

/// Bug 2 sibling: same expectation for --visible.
#[test]
#[serial_test::file_serial(calculator)]
fn visible_filter_returns_tree_not_error() {
    let pid = launch_calculator_backgrounded();

    TestCommand::cargo_bin("accessibility-cli")
        .unwrap()
        .args([
            "--platform",
            "mac",
            "--pid",
            &pid.to_string(),
            "--visible",
            "--llm",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("Failed to build accessibility tree").not())
        .stdout(predicate::str::contains("Calculator"));
}

/// Bug 3: tree-mode header must identify the platform as macOS, not "Unknown".
#[test]
#[serial_test::file_serial(calculator)]
fn tree_header_says_macos() {
    let pid = launch_calculator_backgrounded();

    TestCommand::cargo_bin("accessibility-cli")
        .unwrap()
        .args(["--platform", "mac", "--pid", &pid.to_string()])
        .assert()
        .success()
        .stdout(predicate::str::contains("=== macOS Accessibility Tree ==="))
        .stdout(predicate::str::contains("Unknown Accessibility Tree").not());
}

/// Bug 4 + end-to-end click chain: `--listen --pid <X>` must scope to that
/// PID. Before the fix the CLI built `ListenerConfig` without `.with_pid(...)`,
/// so events from every process streamed in.
///
/// We listen to PID 1 (launchd / init — never emits AX events) and then drive
/// Calculator through a real arithmetic chain (1+2=3, then verify the display).
/// This single test covers two regressions at once:
///   * The listener subprocess must report zero events from PID 1.
///   * Clicks against a backgrounded Calculator must actually compute, proving
///     that the bug-1 backgrounded tree fix kept the click path working.
#[test]
#[serial_test::file_serial(calculator)]
fn listen_pid_filter_scopes_event_stream() {
    let calc_pid = launch_calculator_backgrounded();
    reset_calculator_display(calc_pid);

    // Spawn the CLI with --listen --pid 1. PID 1 has no AX surface; a working
    // filter means zero event rows. A broken filter (the old behavior) would
    // pick up FOCUS_CHANGED / VALUE_CHANGED events from any active app.
    let mut child = Command::new(env!("CARGO_BIN_EXE_accessibility-cli"))
        .args(["--platform", "mac", "--pid", "1", "--listen"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn accessibility-cli --listen");

    let stdout = child.stdout.take().expect("child stdout");
    let captured = Arc::new(Mutex::new(String::new()));
    let captured_cl = Arc::clone(&captured);
    let reader = thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut s = stdout;
        loop {
            match s.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut g) = captured_cl.lock() {
                        g.push_str(&String::from_utf8_lossy(&buf[..n]));
                    }
                }
            }
        }
    });

    // Give the listener a moment to register.
    thread::sleep(Duration::from_millis(500));

    // Drive Calculator: compute 1 + 2 = 3. Each click verifies its own
    // success via the CLI's exit code (assert().success()).
    for desc in ["1", "Add", "2", "Equals"] {
        TestCommand::cargo_bin("accessibility-cli")
            .unwrap()
            .args([
                "--platform",
                "mac",
                "--pid",
                &calc_pid.to_string(),
                "--click",
                &format!("Button[description=\"{desc}\"]"),
                "--timeout",
                "5000",
            ])
            .assert()
            .success();
    }

    // Verify the math actually computed in the backgrounded Calculator.
    // The display contains LRM/RTL marks around digits, so we just look for
    // the bare result text inside the value column.
    let result_assert = TestCommand::cargo_bin("accessibility-cli")
        .unwrap()
        .args([
            "--platform",
            "mac",
            "--pid",
            &calc_pid.to_string(),
            "--query",
            "Text",
            "--timeout",
            "5000",
        ])
        .assert()
        .success();
    let result_out = String::from_utf8_lossy(&result_assert.get_output().stdout).into_owned();
    assert!(
        result_out.contains('3'),
        "expected Calculator to display '3' after 1+2=; got:\n{result_out}"
    );

    // Let events propagate to the listener.
    thread::sleep(Duration::from_millis(500));

    // Stop the listener.
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();

    let out = captured.lock().unwrap().clone();
    // Event rows look like "[N] FOCUS_CHANGED ...", "[N] VALUE_CHANGED ...", etc.
    // The header "Starting accessibility event listener on macOS..." is fine.
    let event_lines: Vec<&str> = out
        .lines()
        .filter(|l| {
            l.contains("FOCUS_CHANGED")
                || l.contains("VALUE_CHANGED")
                || l.contains("TITLE_CHANGED")
                || l.contains("WINDOW_FOCUS_CHANGED")
        })
        .collect();
    assert!(
        event_lines.is_empty(),
        "expected zero events when listening to PID 1; got {} event lines:\n{}",
        event_lines.len(),
        event_lines.join("\n")
    );
}

/// Click whichever clear button Calculator currently exposes ("All Clear" when
/// display is 0, "Clear" otherwise). Best-effort — if neither click succeeds
/// the test will still proceed against whatever state Calculator is in.
fn reset_calculator_display(pid: u32) {
    for desc in ["All Clear", "Clear"] {
        let ok = TestCommand::cargo_bin("accessibility-cli")
            .unwrap()
            .args([
                "--platform",
                "mac",
                "--pid",
                &pid.to_string(),
                "--click",
                &format!("Button[description=\"{desc}\"]"),
                "--timeout",
                "2000",
            ])
            .assert();
        if ok.try_success().is_ok() {
            return;
        }
    }
}

/// Regression: --press accepts a CSS-like query and drives the same AX action
/// chain that --click does on macOS. Before the refactor --press took a numeric
/// ID and was iOS-only, so this exact invocation would have been rejected with
/// "iOS-only flags ... require --platform ios" before parsing the query.
#[test]
#[serial_test::file_serial(calculator)]
fn press_with_query_clicks_calculator_button() {
    let pid = launch_calculator_backgrounded();
    reset_calculator_display(pid);

    for desc in ["3", "Add", "4", "Equals"] {
        TestCommand::cargo_bin("accessibility-cli")
            .unwrap()
            .args([
                "--platform",
                "mac",
                "--pid",
                &pid.to_string(),
                "--press",
                &format!("Button[description=\"{desc}\"]"),
                "--timeout",
                "5000",
            ])
            .assert()
            .success();
    }

    let assert = TestCommand::cargo_bin("accessibility-cli")
        .unwrap()
        .args([
            "--platform",
            "mac",
            "--pid",
            &pid.to_string(),
            "--query",
            "Text",
            "--timeout",
            "5000",
        ])
        .assert()
        .success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        out.contains('7'),
        "expected --press chain to compute 3+4=7 on backgrounded Calculator; got:\n{out}"
    );
}

/// Bug 8: --focus on a Calculator button must not error with "Action Focus
/// not supported on macOS".
#[test]
#[serial_test::file_serial(calculator)]
fn focus_button_does_not_error() {
    let pid = launch_calculator_backgrounded();

    let assert = TestCommand::cargo_bin("accessibility-cli")
        .unwrap()
        .args([
            "--platform",
            "mac",
            "--pid",
            &pid.to_string(),
            "--focus",
            "Button[description=\"5\"]",
        ])
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        !stderr.contains("Action Focus not supported"),
        "expected no AX-action error; got:\n{stderr}"
    );

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        stdout.contains("Focused element"),
        "expected 'Focused element' in stdout; got:\n{stdout}"
    );
}
