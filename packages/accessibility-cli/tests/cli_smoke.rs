use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;

#[test]
fn help_mentions_primary_command() {
    let mut cmd = Command::cargo_bin("accessibility-cli").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("accessibility-cli"))
        .stdout(predicate::str::contains("--platform"));
}

#[test]
fn invalid_platform_fails_before_touching_accessibility_backend() {
    let mut cmd = Command::cargo_bin("accessibility-cli").unwrap();
    cmd.args(["--platform", "not-a-platform"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid value"));
}

#[test]
fn operational_flags_parse_before_backend_startup() {
    let no_adb_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-no-adb");

    let cases: &[&[&str]] = &[
        &["--platform", "android", "--json", "--timeout", "0"],
        &["--platform", "android", "--llm", "--timeout", "0"],
        &[
            "--platform",
            "android",
            "--llm-query",
            "--query",
            "Button",
            "--timeout",
            "0",
        ],
        &[
            "--platform",
            "android",
            "--query",
            "Button",
            "--timeout",
            "0",
        ],
        &[
            "--platform",
            "android",
            "--click",
            "Button",
            "--timeout",
            "0",
        ],
        &[
            "--platform",
            "android",
            "--type",
            "EditText",
            "hello",
            "--timeout",
            "0",
        ],
        &[
            "--platform",
            "android",
            "--key",
            "enter",
            "EditText",
            "--timeout",
            "0",
        ],
        &[
            "--platform",
            "android",
            "--query",
            "Button",
            "--timeout",
            "25",
            "--poll-interval",
            "5",
        ],
    ];

    for args in cases {
        let mut cmd = Command::cargo_bin("accessibility-cli").unwrap();
        cmd.env("PATH", &no_adb_path)
            .args(*args)
            .assert()
            .failure()
            .stderr(predicate::str::contains("ADB binary not found"));
    }
}

#[test]
fn ios_only_flag_rejected_on_other_platform() {
    // Regression for the silent-ignore bug: --tap is iOS-only, must error on mac.
    let mut cmd = Command::cargo_bin("accessibility-cli").unwrap();
    cmd.args(["--platform", "mac", "--tap", "100,100"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("iOS-only flags"));
}

#[test]
fn adb_flag_rejected_on_non_android_platform() {
    let mut cmd = Command::cargo_bin("accessibility-cli").unwrap();
    cmd.args(["--platform", "mac", "--adb-back"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--adb-* flags require --platform android"));
}

#[test]
fn adb_swipe_invalid_duration_rejected() {
    // Regression for silently-defaulted duration: 'abc' must error, not run at 300ms.
    let mut cmd = Command::cargo_bin("accessibility-cli").unwrap();
    cmd.args(["--platform", "android", "--adb-swipe", "1,2,3,4,abc"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid duration_ms"));
}

#[test]
fn adb_long_press_invalid_duration_rejected() {
    let mut cmd = Command::cargo_bin("accessibility-cli").unwrap();
    cmd.args(["--platform", "android", "--adb-long-press", "1,2,xyz"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid duration_ms"));
}
