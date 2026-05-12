use super::RunningApplication;
use objc2_application_services::AXIsProcessTrusted;

pub fn is_process_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
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
