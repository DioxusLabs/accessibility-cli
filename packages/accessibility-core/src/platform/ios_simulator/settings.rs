//! Simulator-wide UI settings.
//!
//! These go through `simctl ui`, which is the same mechanism Xcode's Devices
//! window uses. Only the three options simctl actually implements are exposed:
//! appearance, increase contrast, and content size.
//!
//! The Devices window also offers reduce-motion, colour filters, transparency
//! and VoiceOver, but simctl has no verb for those — they require a helper
//! binary spawned *inside* the simulator that drives the private
//! libAccessibility setters. That is a meaningfully larger piece of work and
//! is deliberately not attempted here.

use std::{
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

const SIMCTL_TIMEOUT: Duration = Duration::from_secs(2);
const SIMCTL_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Content size categories, smallest to largest.
///
/// The five `accessibility-*` entries are the extended range that only appears
/// once a user opts into larger accessibility text.
pub const CONTENT_SIZES: &[&str] = &[
    "extra-small",
    "small",
    "medium",
    "large",
    "extra-large",
    "extra-extra-large",
    "extra-extra-extra-large",
    "accessibility-medium",
    "accessibility-large",
    "accessibility-extra-large",
    "accessibility-extra-extra-large",
    "accessibility-extra-extra-extra-large",
];

pub const APPEARANCES: &[&str] = &["light", "dark"];
pub const TOGGLE: &[&str] = &["enabled", "disabled"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingKey {
    Appearance,
    IncreaseContrast,
    ContentSize,
}

impl SettingKey {
    /// The `simctl ui` subcommand for this setting.
    fn verb(self) -> &'static str {
        match self {
            SettingKey::Appearance => "appearance",
            SettingKey::IncreaseContrast => "increase_contrast",
            SettingKey::ContentSize => "content_size",
        }
    }

    pub fn allowed_values(self) -> &'static [&'static str] {
        match self {
            SettingKey::Appearance => APPEARANCES,
            SettingKey::IncreaseContrast => TOGGLE,
            SettingKey::ContentSize => CONTENT_SIZES,
        }
    }

    pub fn all() -> [SettingKey; 3] {
        [
            SettingKey::Appearance,
            SettingKey::IncreaseContrast,
            SettingKey::ContentSize,
        ]
    }
}

/// One setting and its current value, as reported by the simulator.
#[derive(Debug, Clone, Serialize)]
pub struct Setting {
    pub key: SettingKey,
    /// Current value, or `unsupported`/`unknown` if the runtime says so.
    pub value: String,
    pub allowed: &'static [&'static str],
}

/// Read every supported setting from the device.
pub fn read_all(udid: &str) -> Vec<Setting> {
    SettingKey::all()
        .into_iter()
        .map(|key| Setting {
            key,
            // A failed read is reported as unknown rather than failing the
            // whole request; one unsupported option should not blank the UI.
            value: read(udid, key).unwrap_or_else(|_| "unknown".to_string()),
            allowed: key.allowed_values(),
        })
        .collect()
}

pub fn read(udid: &str, key: SettingKey) -> Result<String> {
    let output = simctl(&["ui", udid, key.verb()])?;
    Ok(output.trim().to_string())
}

pub fn write(udid: &str, key: SettingKey, value: &str) -> Result<String> {
    // `content_size` also accepts increment/decrement, which are not in the
    // reported value set but are the ergonomic way to drive it from a UI.
    let stepping =
        matches!(key, SettingKey::ContentSize) && matches!(value, "increment" | "decrement");

    if !stepping && !key.allowed_values().contains(&value) {
        return Err(anyhow!(
            "'{value}' is not valid for {:?}; expected one of {}",
            key,
            key.allowed_values().join(", ")
        ));
    }

    simctl(&["ui", udid, key.verb(), value])?;
    read(udid, key)
}

fn simctl(args: &[&str]) -> Result<String> {
    let mut child = Command::new("xcrun")
        .arg("simctl")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to run xcrun simctl")?;

    let deadline = Instant::now() + SIMCTL_TIMEOUT;
    loop {
        if child
            .try_wait()
            .context("failed to wait for xcrun simctl")?
            .is_some()
        {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().context("failed to terminate xcrun simctl")?;
            child
                .wait()
                .context("failed to reap timed out xcrun simctl")?;
            return Err(anyhow!(
                "simctl {} timed out after {} seconds",
                args.join(" "),
                SIMCTL_TIMEOUT.as_secs()
            ));
        }
        thread::sleep(SIMCTL_POLL_INTERVAL);
    }

    let output = child
        .wait_with_output()
        .context("failed to collect xcrun simctl output")?;
    if !output.status.success() {
        return Err(anyhow!(
            "simctl {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_values_outside_the_allowed_set() {
        let error = write("no-such-device", SettingKey::Appearance, "chartreuse")
            .expect_err("invalid appearance should be rejected");
        // Rejected locally, without ever shelling out to simctl.
        assert!(error.to_string().contains("chartreuse"));
    }

    #[test]
    fn content_size_accepts_stepping_verbs() {
        // These are not reported values, so they must be allowed explicitly.
        assert!(!CONTENT_SIZES.contains(&"increment"));
        for value in ["increment", "decrement"] {
            let error = write("no-such-device", SettingKey::ContentSize, value)
                .expect_err("no such device");
            assert!(
                !error.to_string().contains("is not valid"),
                "{value} should reach simctl rather than being rejected"
            );
        }
    }

    #[test]
    fn every_key_has_values_and_a_distinct_verb() {
        let mut verbs = Vec::new();
        for key in SettingKey::all() {
            assert!(!key.allowed_values().is_empty());
            verbs.push(key.verb());
        }
        verbs.sort_unstable();
        verbs.dedup();
        assert_eq!(verbs.len(), SettingKey::all().len());
    }
}
