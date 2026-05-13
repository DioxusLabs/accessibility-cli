use super::RunningApplication;
use objc2_application_services::AXIsProcessTrusted;
use std::path::PathBuf;

pub fn is_process_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Return the bundle filesystem path for a running PID, if any.
pub fn bundle_path_for_pid(pid: u32) -> Option<PathBuf> {
    use objc2_app_kit::NSRunningApplication;

    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid as i32)?;
    let url = app.bundleURL()?;
    let path = url.path()?;
    Some(PathBuf::from(path.to_string()))
}

/// Whether the running app is built on Chromium (Electron, Chrome, Edge,
/// Brave, etc.). Detected by looking for known Chromium frameworks in the
/// app's bundle. Used to choose between AXPress (reliable for native AppKit
/// controls) and synthetic mouse clicks (required for Chromium-hosted web
/// elements, where the AX-to-DOM bridge silently drops AXPress).
pub fn is_chromium_based_app(pid: u32) -> bool {
    let Some(bundle) = bundle_path_for_pid(pid) else {
        return false;
    };
    let frameworks = bundle.join("Contents").join("Frameworks");
    const CHROMIUM_FRAMEWORKS: &[&str] = &[
        "Electron Framework.framework",
        "Google Chrome Framework.framework",
        "Chromium Framework.framework",
        "Microsoft Edge Framework.framework",
        "Brave Browser Framework.framework",
    ];
    CHROMIUM_FRAMEWORKS
        .iter()
        .any(|name| frameworks.join(name).exists())
}

pub fn frontmost_application_pid() -> Option<u32> {
    use objc2::rc::Retained;
    use objc2_app_kit::{NSRunningApplication, NSWorkspace};

    let workspace = NSWorkspace::sharedWorkspace();
    let frontmost: Option<Retained<NSRunningApplication>> = workspace.frontmostApplication();

    frontmost
        .map(|app| app.processIdentifier())
        .filter(|pid| *pid > 0)
        .map(|pid| pid as u32)
}

pub fn running_applications() -> Vec<RunningApplication> {
    use objc2_app_kit::NSWorkspace;

    let workspace = NSWorkspace::sharedWorkspace();
    workspace
        .runningApplications()
        .iter()
        .filter_map(|app| {
            let pid = app.processIdentifier();
            if pid <= 0 {
                return None;
            }

            Some(RunningApplication {
                pid: pid as u32,
                localized_name: app.localizedName().map(|name| name.to_string()),
                activation_policy: app.activationPolicy().0,
            })
        })
        .collect()
}
