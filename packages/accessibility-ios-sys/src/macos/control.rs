//! Typed direct controls for a booted iOS Simulator.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Result, anyhow};
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::msg_send;
use objc2::runtime::{AnyObject, Bool, Sel};
use objc2_foundation::{NSMutableArray, NSMutableDictionary, NSNumber, NSString};

use super::common::{find_booted_device, nsstring_to_string_static};

/// Typed direct control handle for one booted simulator.
///
/// Setting getters and setters perform synchronous CoreSimulator IPC and may
/// block. Keep this handle on a dedicated worker rather than an async runtime
/// thread.
pub struct SimulatorDevice {
    device: *mut AnyObject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum SimulatorAppearance {
    Light = 1,
    Dark = 2,
}

impl TryFrom<i64> for SimulatorAppearance {
    type Error = anyhow::Error;

    fn try_from(value: i64) -> Result<Self> {
        match value {
            1 => Ok(Self::Light),
            2 => Ok(Self::Dark),
            _ => Err(anyhow!("unknown simulator appearance value {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum SimulatorIncreaseContrast {
    Disabled = 1,
    Enabled = 2,
}

impl TryFrom<i64> for SimulatorIncreaseContrast {
    type Error = anyhow::Error;

    fn try_from(value: i64) -> Result<Self> {
        match value {
            1 => Ok(Self::Disabled),
            2 => Ok(Self::Enabled),
            _ => Err(anyhow!("unknown simulator contrast value {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum SimulatorContentSize {
    ExtraSmall = 1,
    Small = 2,
    Medium = 3,
    Large = 4,
    ExtraLarge = 5,
    ExtraExtraLarge = 6,
    ExtraExtraExtraLarge = 7,
    AccessibilityMedium = 8,
    AccessibilityLarge = 9,
    AccessibilityExtraLarge = 10,
    AccessibilityExtraExtraLarge = 11,
    AccessibilityExtraExtraExtraLarge = 12,
}

impl SimulatorContentSize {
    pub fn index(self) -> usize {
        self as usize - 1
    }

    pub fn step(self, delta: i64) -> Self {
        Self::try_from((self as i64 + delta).clamp(1, 12))
            .expect("clamped content size should be valid")
    }
}

impl TryFrom<i64> for SimulatorContentSize {
    type Error = anyhow::Error;

    fn try_from(value: i64) -> Result<Self> {
        match value {
            1 => Ok(Self::ExtraSmall),
            2 => Ok(Self::Small),
            3 => Ok(Self::Medium),
            4 => Ok(Self::Large),
            5 => Ok(Self::ExtraLarge),
            6 => Ok(Self::ExtraExtraLarge),
            7 => Ok(Self::ExtraExtraExtraLarge),
            8 => Ok(Self::AccessibilityMedium),
            9 => Ok(Self::AccessibilityLarge),
            10 => Ok(Self::AccessibilityExtraLarge),
            11 => Ok(Self::AccessibilityExtraExtraLarge),
            12 => Ok(Self::AccessibilityExtraExtraExtraLarge),
            _ => Err(anyhow!("unknown simulator content size value {value}")),
        }
    }
}

unsafe impl Send for SimulatorDevice {}

impl Drop for SimulatorDevice {
    fn drop(&mut self) {
        unsafe { objc2::ffi::objc_release(self.device) };
    }
}

impl SimulatorDevice {
    pub fn for_device(udid: Option<&str>) -> Result<Self> {
        crate::frameworks::load_coresimulator_framework()?;
        let device = unsafe { find_booted_device(udid)? };
        Ok(unsafe { Self::retaining(device) })
    }

    pub(super) unsafe fn retaining(device: *mut AnyObject) -> Self {
        let device = unsafe { objc2::ffi::objc_retain(device) };
        Self { device }
    }

    pub fn appearance(&self) -> Result<SimulatorAppearance> {
        self.require_selector(objc2::sel!(currentUIInterfaceStyle))?;
        let value: i64 = unsafe { msg_send![self.device, currentUIInterfaceStyle] };
        SimulatorAppearance::try_from(value)
    }

    pub fn set_appearance(&self, appearance: SimulatorAppearance) -> Result<()> {
        self.require_selector(objc2::sel!(setUIInterfaceStyle:error:))?;
        let mut error = std::ptr::null_mut();
        let success: Bool = unsafe {
            msg_send![self.device, setUIInterfaceStyle: appearance as i64, error: &mut error]
        };
        Self::bool_result(success, error, "set simulator appearance")
    }

    pub fn content_size(&self) -> Result<SimulatorContentSize> {
        self.require_selector(objc2::sel!(currentContentSizeCategory))?;
        let value: i64 = unsafe { msg_send![self.device, currentContentSizeCategory] };
        SimulatorContentSize::try_from(value)
    }

    pub fn set_content_size(&self, size: SimulatorContentSize) -> Result<()> {
        self.require_selector(objc2::sel!(setContentSizeCategory:error:))?;
        let mut error = std::ptr::null_mut();
        let success: Bool = unsafe {
            msg_send![self.device, setContentSizeCategory: size as i64, error: &mut error]
        };
        Self::bool_result(success, error, "set simulator content size")
    }

    pub fn increase_contrast(&self) -> Result<SimulatorIncreaseContrast> {
        self.require_selector(objc2::sel!(currentIncreaseContrastMode))?;
        let value: i64 = unsafe { msg_send![self.device, currentIncreaseContrastMode] };
        SimulatorIncreaseContrast::try_from(value)
    }

    pub fn set_increase_contrast(&self, contrast: SimulatorIncreaseContrast) -> Result<()> {
        self.require_selector(objc2::sel!(setIncreaseContrastEnabled:error:))?;
        let mut error = std::ptr::null_mut();
        let enabled = contrast == SimulatorIncreaseContrast::Enabled;
        let success: Bool = unsafe {
            msg_send![
                self.device,
                setIncreaseContrastEnabled: Bool::from(enabled),
                error: &mut error
            ]
        };
        Self::bool_result(success, error, "set simulator increase contrast")
    }

    /// Restart the guest accessibility bridge and wait for launchd to stop it.
    ///
    /// Starts:
    ///
    /// 1. One guest `launchctl stop` process through CoreSimulator.
    /// 2. Spawn-completion and process-termination callbacks on the control queue.
    pub(super) fn restart_accessibility_bridge(&self) -> Result<()> {
        const TIMEOUT: Duration = Duration::from_secs(2);

        let selector = objc2::sel!(spawnAsyncWithPath:options:terminationQueue:terminationHandler:completionQueue:completionHandler:);
        let responds: Bool = unsafe { msg_send![self.device, respondsToSelector: selector] };
        if !responds.as_bool() {
            return Err(anyhow!(
                "CoreSimulator does not support asynchronous process spawning"
            ));
        }

        let runtime: *mut AnyObject = unsafe { msg_send![self.device, runtime] };
        if runtime.is_null() {
            return Err(anyhow!("Simulator runtime is unavailable"));
        }
        let root: *mut AnyObject = unsafe { msg_send![runtime, root] };
        let root = unsafe { nsstring_to_string_static(root) }
            .ok_or_else(|| anyhow!("Simulator runtime root is unavailable"))?;
        let launch_path = PathBuf::from(root).join("bin/launchctl");
        let launch_path = launch_path
            .to_str()
            .ok_or_else(|| anyhow!("Simulator launchctl path is not valid UTF-8"))?;

        let arguments: objc2::rc::Retained<NSMutableArray<NSString>> = NSMutableArray::new();
        for argument in [launch_path, "stop", "com.apple.CoreSimulator.bridge"] {
            arguments.addObject(&NSString::from_str(argument));
        }
        let options: objc2::rc::Retained<NSMutableDictionary<NSString, AnyObject>> =
            NSMutableDictionary::new();
        let arguments_key = NSString::from_str("arguments");
        let standalone_key = NSString::from_str("standalone");
        unsafe {
            let _: () = msg_send![
                &*options,
                setObject: &*arguments,
                forKey: &*arguments_key
            ];
            let standalone = NSNumber::new_bool(false);
            let _: () = msg_send![
                &*options,
                setObject: &*standalone,
                forKey: &*standalone_key
            ];
        }

        let process = ProcessSpawn::new();
        let completion = Arc::clone(&process);
        let completion_block = RcBlock::new(move |error: *mut AnyObject, pid: i32| {
            let result = if error.is_null() {
                Ok(pid)
            } else {
                let description: *mut AnyObject = unsafe { msg_send![error, localizedDescription] };
                Err(unsafe { nsstring_to_string_static(description) }
                    .unwrap_or_else(|| "unknown CoreSimulator spawn error".to_string()))
            };
            completion.complete(result);
        });

        let termination = Arc::clone(&process);
        let termination_block = RcBlock::new(move |status: i32| termination.terminate(status));

        let queue = Self::callback_queue();
        let launch_path = NSString::from_str(launch_path);
        unsafe {
            let _: () = msg_send![
                self.device,
                spawnAsyncWithPath: &*launch_path,
                options: &*options,
                terminationQueue: queue,
                terminationHandler: &*termination_block,
                completionQueue: queue,
                completionHandler: &*completion_block
            ];
        }

        process.wait_for_spawn(TIMEOUT)?;
        let status = process.wait_for_termination(TIMEOUT)?;
        if libc::WIFEXITED(status) {
            let code = libc::WEXITSTATUS(status);
            if code == 0 || code == libc::ESRCH {
                return Ok(());
            }
            return Err(anyhow!("Simulator launchctl exited with status {code}"));
        }
        if libc::WIFSIGNALED(status) {
            return Err(anyhow!(
                "Simulator launchctl terminated by signal {}",
                libc::WTERMSIG(status)
            ));
        }
        Err(anyhow!("Simulator launchctl returned wait status {status}"))
    }

    fn callback_queue() -> *mut AnyObject {
        static QUEUE: OnceLock<DispatchRetained<DispatchQueue>> = OnceLock::new();
        let queue = QUEUE.get_or_init(|| {
            DispatchQueue::new(
                "com.accessibility_cli.simulator.control",
                DispatchQueueAttr::SERIAL,
            )
        });
        DispatchRetained::as_ptr(queue).as_ptr().cast::<AnyObject>()
    }

    fn require_selector(&self, selector: Sel) -> Result<()> {
        let responds: Bool = unsafe { msg_send![self.device, respondsToSelector: selector] };
        if responds.as_bool() {
            Ok(())
        } else {
            Err(anyhow!(
                "CoreSimulator does not support selector {}",
                selector.name().to_string_lossy()
            ))
        }
    }

    fn bool_result(success: Bool, error: *mut AnyObject, operation: &str) -> Result<()> {
        if success.as_bool() {
            return Ok(());
        }
        let detail = unsafe {
            (!error.is_null())
                .then(|| {
                    let description: *mut AnyObject = msg_send![error, localizedDescription];
                    nsstring_to_string_static(description)
                })
                .flatten()
        };
        Err(anyhow!(
            "Failed to {operation}: {}",
            detail.as_deref().unwrap_or("no error detail")
        ))
    }
}

#[derive(Debug, Default)]
struct ProcessState {
    result: Option<std::result::Result<i32, String>>,
    status: Option<i32>,
}

#[derive(Debug)]
struct ProcessSpawn {
    state: Mutex<ProcessState>,
    changed: Condvar,
}

impl ProcessSpawn {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ProcessState::default()),
            changed: Condvar::new(),
        })
    }

    fn complete(&self, result: std::result::Result<i32, String>) {
        self.state.lock().unwrap().result = Some(result);
        self.changed.notify_all();
    }

    fn terminate(&self, status: i32) {
        self.state.lock().unwrap().status = Some(status);
        self.changed.notify_all();
    }

    fn wait_for_spawn(&self, timeout: Duration) -> Result<i32> {
        let state = self.state.lock().unwrap();
        let (state, wait) = self
            .changed
            .wait_timeout_while(state, timeout, |state| state.result.is_none())
            .unwrap();
        if wait.timed_out() {
            return Err(anyhow!("CoreSimulator process spawn timed out"));
        }
        state
            .result
            .as_ref()
            .expect("spawn completion should be populated")
            .as_ref()
            .copied()
            .map_err(|error| anyhow!(error.clone()))
    }

    fn wait_for_termination(&self, timeout: Duration) -> Result<i32> {
        let state = self.state.lock().unwrap();
        let (state, wait) = self
            .changed
            .wait_timeout_while(state, timeout, |state| state.status.is_none())
            .unwrap();
        if wait.timed_out() {
            return Err(anyhow!("Simulator process did not exit"));
        }
        Ok(state
            .status
            .expect("termination status should be populated"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_values_reject_unknown_raw_values() {
        assert!(SimulatorAppearance::try_from(0).is_err());
        assert!(SimulatorAppearance::try_from(3).is_err());
        assert!(SimulatorIncreaseContrast::try_from(0).is_err());
        assert!(SimulatorIncreaseContrast::try_from(3).is_err());
        assert!(SimulatorContentSize::try_from(0).is_err());
        assert!(SimulatorContentSize::try_from(13).is_err());
    }

    #[test]
    fn content_size_steps_stay_in_range() {
        assert_eq!(
            SimulatorContentSize::ExtraSmall.step(-1),
            SimulatorContentSize::ExtraSmall
        );
        assert_eq!(
            SimulatorContentSize::ExtraSmall.step(1),
            SimulatorContentSize::Small
        );
        assert_eq!(
            SimulatorContentSize::AccessibilityExtraExtraExtraLarge.step(1),
            SimulatorContentSize::AccessibilityExtraExtraExtraLarge
        );
    }

    #[test]
    fn process_callbacks_share_one_wait_state() {
        let process = ProcessSpawn::new();
        let callback = Arc::clone(&process);
        let spawn = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            callback.complete(Ok(42));
        });
        assert_eq!(process.wait_for_spawn(Duration::from_secs(1)).unwrap(), 42);
        spawn.join().unwrap();

        let callback = Arc::clone(&process);
        let terminate = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            callback.terminate(0);
        });
        assert_eq!(
            process
                .wait_for_termination(Duration::from_secs(1))
                .unwrap(),
            0
        );
        terminate.join().unwrap();
    }
}
