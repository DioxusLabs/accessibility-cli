use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;

fn no_adb_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-no-adb")
}

#[test]
fn help_mentions_primary_commands() {
    let mut cmd = Command::cargo_bin("accessibility-cli").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("accessibility-cli"))
        .stdout(predicate::str::contains("tree"))
        .stdout(predicate::str::contains("query"));

    let mut cmd = Command::cargo_bin("accessibility-cli").unwrap();
    cmd.args(["tree", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--platform"));
}

#[test]
fn invalid_platform_fails_before_touching_accessibility_backend() {
    let mut cmd = Command::cargo_bin("accessibility-cli").unwrap();
    cmd.args(["tree", "--platform", "not-a-platform"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid value"));
}

#[test]
fn old_flat_operational_flags_are_rejected() {
    let cases: &[&[&str]] = &[
        &["--platform", "android", "--query", "Button"],
        &["--platform", "android", "--click", "Button"],
        &["--platform", "android", "--adb-back"],
        &["--platform", "ios", "--hid-tap", "100,200"],
        &["tree", "--platform", "android", "--llm"],
        &["tree", "--platform", "android", "--json"],
    ];

    for args in cases {
        let mut cmd = Command::cargo_bin("accessibility-cli").unwrap();
        cmd.args(*args)
            .assert()
            .failure()
            .stderr(predicate::str::contains("unexpected argument"));
    }
}

#[test]
fn operational_subcommands_parse_before_backend_startup() {
    let cases: &[&[&str]] = &[
        &["tree", "--platform", "android", "--format", "json"],
        &["tree", "--platform", "android", "--format", "llm"],
        &["query", "Button", "--platform", "android", "--timeout", "0"],
        &[
            "query",
            "Button",
            "--platform",
            "android",
            "--timeout",
            "25",
            "--poll-interval",
            "5",
        ],
        &["click", "Button", "--platform", "android", "--timeout", "0"],
        &["press", "Button", "--platform", "android", "--timeout", "0"],
        &[
            "type",
            "EditText",
            "hello",
            "--platform",
            "android",
            "--timeout",
            "0",
        ],
        &[
            "key",
            "enter",
            "EditText",
            "--platform",
            "android",
            "--timeout",
            "0",
        ],
    ];

    for args in cases {
        let mut cmd = Command::cargo_bin("accessibility-cli").unwrap();
        cmd.env("PATH", no_adb_path())
            .args(*args)
            .assert()
            .failure()
            .stderr(predicate::str::contains("ADB binary not found"));
    }
}

#[test]
fn target_flags_are_rejected_on_wrong_platform() {
    let cases: &[(&[&str], &str)] = &[
        (
            &["tree", "--platform", "android", "--pid", "123"],
            "--pid is valid only for mac, win, or linux",
        ),
        (
            &["tree", "--platform", "mac", "--serial", "ABC"],
            "--serial requires --platform android",
        ),
        (
            &["tree", "--platform", "ios", "--pid", "123"],
            "--pid is valid only for mac, win, or linux",
        ),
        (
            &["tree", "--platform", "android", "--udid", "ABC"],
            "--udid requires --platform ios",
        ),
    ];

    for (args, message) in cases {
        let mut cmd = Command::cargo_bin("accessibility-cli").unwrap();
        cmd.args(*args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(*message));
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
#[test]
fn pid_target_app_operations_require_pid_before_backend_startup() {
    let platform = if cfg!(target_os = "macos") {
        "mac"
    } else if cfg!(target_os = "windows") {
        "win"
    } else {
        "linux"
    };

    let cases: Vec<Vec<&str>> = vec![
        vec!["tree", "--platform", platform, "--format", "llm"],
        vec!["click", "Button", "--platform", platform, "--timeout", "0"],
        vec![
            "key",
            "enter",
            "TextField",
            "--platform",
            platform,
            "--timeout",
            "0",
        ],
        vec!["listen", "--platform", platform],
    ];

    for args in cases {
        let mut cmd = Command::cargo_bin("accessibility-cli").unwrap();
        cmd.args(args).assert().failure().stderr(
            predicate::str::contains("app operations require --pid")
                .and(predicate::str::contains("list-windows")),
        );
    }
}

#[test]
fn platform_specific_actions_fail_before_backend_startup() {
    let cases: &[(&[&str], &str)] = &[
        (
            &["tap", "100,100", "--platform", "mac"],
            "tap is supported only",
        ),
        (
            &["button", "back", "--platform", "mac"],
            "button is supported only",
        ),
        (
            &["launch", "com.example.app", "--platform", "ios"],
            "launch is supported only on Android",
        ),
        (
            &["listen", "--platform", "android"],
            "listen is supported only",
        ),
    ];

    for (args, message) in cases {
        let mut cmd = Command::cargo_bin("accessibility-cli").unwrap();
        cmd.args(*args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(*message));
    }
}

#[test]
fn invalid_values_are_rejected_by_clap() {
    let cases: &[&[&str]] = &[
        &["tree", "--platform", "android", "--format", "llm-query"],
        &["swipe", "1,2,3", "--platform", "android"],
        &[
            "swipe",
            "1,2,3,4",
            "--platform",
            "android",
            "--duration",
            "abc",
        ],
        &[
            "long-press",
            "1,2",
            "--platform",
            "android",
            "--duration",
            "xyz",
        ],
        &[
            "mouse-click",
            "1,2",
            "--button",
            "not-a-button",
            "--platform",
            "mac",
        ],
    ];

    for args in cases {
        let mut cmd = Command::cargo_bin("accessibility-cli").unwrap();
        cmd.args(*args)
            .assert()
            .failure()
            .stderr(predicate::str::contains("invalid"));
    }
}
