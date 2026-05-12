use super::*;
use std::collections::HashSet;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static GUI_TEST_LOCK: Mutex<()> = Mutex::new(());

const CHILD_ATTRIBUTES: &[&str] = &[
    "AXChildren",
    "AXVisibleChildren",
    "AXChildrenInNavigationOrder",
    "AXContents",
    "AXRows",
    "AXColumns",
    "AXTabs",
    "AXToolbar",
    "AXSplitters",
    "AXSelectedChildren",
    "AXSelectedRows",
    "AXSelectedColumns",
    "AXWindows",
    "AXMainWindow",
    "AXFocusedWindow",
    "AXFocusedUIElement",
];

struct DialogGuard {
    child: Child,
}

impl DialogGuard {
    fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for DialogGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn launch_dialog() -> DialogGuard {
    let child = Command::new("osascript")
        .args([
            "-e",
            r#"display dialog "accessibility-macos-sys api test" default answer "before" buttons {"OK"} default button "OK""#,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to launch osascript dialog");

    DialogGuard { child }
}

fn walk(root: &AxElement, max_depth: usize) -> Vec<AxElement> {
    fn visit(
        element: &AxElement,
        depth: usize,
        max_depth: usize,
        seen: &mut HashSet<usize>,
        out: &mut Vec<AxElement>,
    ) {
        if depth > max_depth || !seen.insert(element.identity()) {
            return;
        }

        out.push(element.clone());
        for attribute in CHILD_ATTRIBUTES {
            for child in element.attribute_elements(attribute) {
                visit(&child, depth + 1, max_depth, seen, out);
            }
        }
    }

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    visit(root, 0, max_depth, &mut seen, &mut out);
    out
}

fn find_by_role(root: &AxElement, role: &str) -> Option<AxElement> {
    walk(root, 10)
        .into_iter()
        .find(|element| element.attribute_string("AXRole").as_deref() == Some(role))
}

fn find_dialog_elements(root: &AxElement) -> Option<(AxElement, AxElement, AxElement)> {
    let window = find_by_role(root, "AXWindow").or_else(|| find_by_role(root, "AXDialog"))?;
    let text_field = find_by_role(root, "AXTextField")?;
    let button = find_by_role(root, "AXButton")?;

    Some((window, text_field, button))
}

fn wait_for_dialog_elements(root: &AxElement) -> Option<(AxElement, AxElement, AxElement)> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(elements) = find_dialog_elements(root) {
            return Some(elements);
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    None
}

fn assert_png(image: &PngImage) {
    assert!(image.width > 0, "PNG width should be non-zero");
    assert!(image.height > 0, "PNG height should be non-zero");
    assert!(
        image.data.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image should be PNG"
    );
}

fn assert_anyhow_result<T>(result: anyhow::Result<T>) {
    if let Err(error) = result {
        assert!(
            !error.to_string().is_empty(),
            "fallible sys API should report errors"
        );
    }
}

fn assert_ax_result(result: std::result::Result<(), AxErrorCode>) {
    if let Err(error) = result {
        assert!(
            !error.to_string().is_empty(),
            "AX fallible sys API should report errors"
        );
    }
}

fn assert_capture_window_result(result: anyhow::Result<Option<PngImage>>) {
    match result {
        Ok(Some(image)) => assert_png(&image),
        Ok(None) => {}
        Err(error) => assert!(
            !error.to_string().is_empty(),
            "window capture failures should be reported"
        ),
    }
}

fn exercise_element_api(element: &AxElement, point: Point, size: Size) {
    assert!(element.identity() != 0);
    let _ = element.pid();
    let attribute_names = element.attribute_names();
    assert!(
        attribute_names.iter().all(|name| !name.is_empty()),
        "AX attribute names should not contain empty strings"
    );
    let _ = element.has_attribute("AXRole");
    let _ = element.attribute_string("AXRole");
    let _ = element.attribute_bool("AXFocused");
    let _ = element.attribute_point("AXPosition");
    let _ = element.attribute_size("AXSize");
    let _ = element.bounds("AXPosition", "AXSize");
    let _ = element.attribute_elements("AXChildren");
    let action_names = element.action_names();
    assert!(
        action_names.iter().all(|name| !name.is_empty()),
        "AX action names should not contain empty strings"
    );

    let focus_result = element.set_bool_attribute_result("AXFocused", true);
    assert_eq!(
        element.set_bool_attribute("AXFocused", true),
        focus_result.is_success(),
        "bool convenience API should match the raw result wrapper"
    );
    assert_ax_result(element.set_string_attribute("AXValue", "after"));
    assert_ax_result(element.set_point_attribute("AXPosition", point));
    assert_ax_result(element.set_size_attribute("AXSize", size));
    assert_ax_result(element.perform_action("AXRaise"));
    let _ = element.window_id();
}

fn exercise_observer_api(pid: u32, target: &AxElement) {
    match AxObserver::new(pid) {
        Ok(observer) => {
            let notified = AtomicBool::new(false);
            let _ = observer.add_notification(target, "AXValueChanged", &notified);
            observer.add_notifications(target, &["AXTitleChanged"], &notified);

            if let Some(run_loop) = RunLoop::current() {
                let source = observer.run_loop_source();
                run_loop.add_default_source(&source);
                run_default_loop_slice(0.01, true);
                run_loop.remove_default_source(&source);
            }
        }
        Err(error) => assert!(
            !error.to_string().is_empty(),
            "observer creation failures should be reported"
        ),
    }
}

fn exercise_event_api(pid: u32, window_id: Option<WindowId>) {
    let modifiers = ModifierFlags {
        shift: true,
        control: true,
        alt: true,
        meta: true,
    };
    assert_anyhow_result(post_keyboard_event(Some(pid), 0, modifiers, false));
    assert_anyhow_result(post_scroll_event(Some(pid), 0.0, 0.0));
    for button_kind in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
        for event_kind in [
            MouseEventKind::Move,
            MouseEventKind::Down,
            MouseEventKind::Up,
        ] {
            assert_anyhow_result(post_mouse_event(
                Some(pid),
                window_id,
                -1.0,
                -1.0,
                event_kind,
                button_kind,
                0,
                0.0,
            ));
        }
    }
}

#[test]
fn system_wide_element_can_be_constructed() {
    let element = AxElement::system_wide();
    let _ = element.attribute_names();
    let _ = element.identity();
}

#[test]
fn system_wide_attribute_reads_are_repeatable() {
    let element = AxElement::system_wide();
    for _ in 0..3 {
        let _ = element.attribute_string("AXRole");
        let _ = element.attribute_bool("AXFocused");
        let _ = element.attribute_elements("AXFocusedUIElement");
        let _ = element.action_names();
    }
}

#[test]
fn unsupported_attributes_fail_closed() {
    let element = AxElement::system_wide();
    assert!(
        element
            .attribute_string("__accessibility_cli_missing__")
            .is_none()
    );
    assert!(
        element
            .attribute_bool("__accessibility_cli_missing__")
            .is_none()
    );
    assert!(
        element
            .attribute_elements("__accessibility_cli_missing__")
            .is_empty()
    );
}

#[test]
fn ax_errors_are_reported_as_codes() {
    assert!(AxErrorCode::SUCCESS.is_success());
    assert!(!AxErrorCode::FAILURE.is_success());
    assert_eq!(AxErrorCode::FAILURE.to_string(), "AXError(-25200)");
}

#[test]
fn private_window_alpha_fails_closed_for_invalid_window() {
    assert!(!set_window_alpha(WindowId(0), 1.0));
}

#[test]
fn public_api_runs_against_real_dialog_process() {
    let _guard = GUI_TEST_LOCK.lock().expect("GUI test lock poisoned");
    let trusted = is_process_trusted();

    let display_bounds = main_display_bounds();
    assert!(display_bounds.origin.x.is_finite());
    assert!(display_bounds.origin.y.is_finite());
    assert!(display_bounds.size.width.is_finite());
    assert!(display_bounds.size.height.is_finite());
    assert!(display_bounds.size.width >= 0.0);
    assert!(display_bounds.size.height >= 0.0);
    match capture_main_display() {
        Ok(image) => assert_png(&image),
        Err(error) => assert!(
            !error.to_string().is_empty(),
            "display capture failures should be reported"
        ),
    }
    let _mouse = current_mouse_location().expect("mouse location should be readable");
    let _frontmost_pid = frontmost_application_pid();
    let applications = running_applications();
    assert!(
        applications.iter().all(|app| app.pid > 0),
        "NSWorkspace should not return zero-pid applications"
    );

    let dialog = launch_dialog();
    let pid = dialog.pid();
    let app = AxElement::application(pid);
    let system = AxElement::system_wide();
    let probe_point = Point::new(
        display_bounds.origin.x + display_bounds.size.width / 2.0,
        display_bounds.origin.y + display_bounds.size.height / 2.0,
    );
    let probe_size = Size::new(
        display_bounds.size.width.max(1.0),
        display_bounds.size.height.max(1.0),
    );

    exercise_element_api(&app, probe_point, probe_size);
    exercise_element_api(&system, probe_point, probe_size);

    let dialog_elements = if trusted {
        wait_for_dialog_elements(&app)
    } else {
        find_dialog_elements(&app)
    };

    if let Some((window, text_field, button)) = dialog_elements {
        if let Some(reported_pid) = app.pid() {
            assert_eq!(reported_pid, pid);
        }
        assert!(app.identity() != 0);
        assert!(!app.attribute_names().is_empty());
        assert!(app.has_attribute("AXRole"));
        assert_eq!(
            app.attribute_string("AXRole").as_deref(),
            Some("AXApplication")
        );
        assert!(
            app.attribute_bool("__accessibility_macos_sys_missing__")
                .is_none()
        );
        assert!(!app.attribute_elements("AXChildren").is_empty());

        let bounds = window
            .bounds("AXPosition", "AXSize")
            .expect("dialog window should expose bounds");
        assert!(bounds.size.width > 0.0);
        assert!(bounds.size.height > 0.0);
        assert!(window.attribute_point("AXPosition").is_some());
        assert!(window.attribute_size("AXSize").is_some());

        let focus_result = window.set_bool_attribute_result("AXFocused", true);
        assert_eq!(
            window.set_bool_attribute("AXFocused", true),
            focus_result.is_success(),
            "bool convenience API should match the raw result wrapper"
        );

        let _ = window.set_point_attribute("AXPosition", bounds.origin);
        let _ = window.set_size_attribute("AXSize", bounds.size);
        let _ = window.perform_action("AXRaise");

        let hit = system.element_at_position(
            bounds.origin.x + bounds.size.width / 2.0,
            bounds.origin.y + bounds.size.height / 2.0,
        );
        assert!(hit.is_some(), "system hit testing should return an element");

        let window_id = window
            .window_id()
            .expect("dialog window should resolve to a WindowId");
        assert!(set_window_alpha(window_id, 1.0));
        let window_capture = capture_window(window_id)
            .expect("window capture should not error")
            .expect("window capture should return an image");
        assert_png(&window_capture);

        let observer = AxObserver::new(pid).expect("observer creation should succeed");
        let notified = AtomicBool::new(false);
        let notification_result =
            observer.add_notification(&text_field, "AXValueChanged", &notified);
        assert!(
            notification_result.is_success(),
            "AXValueChanged registration failed: {notification_result:?}"
        );
        observer.add_notifications(&button, &["AXTitleChanged"], &notified);

        let run_loop = RunLoop::current().expect("current run loop should be available");
        let source = observer.run_loop_source();
        run_loop.add_default_source(&source);

        text_field
            .set_string_attribute("AXValue", "after")
            .expect("dialog text field value should be writable");
        for _ in 0..10 {
            run_default_loop_slice(0.05, true);
            if notified.load(Ordering::SeqCst) {
                break;
            }
        }
        run_loop.remove_default_source(&source);
        assert!(
            notified.load(Ordering::SeqCst),
            "text field value write should trigger AXValueChanged"
        );

        post_keyboard_event(Some(pid), 0, ModifierFlags::default(), false)
            .expect("per-pid key-up post should succeed");
        post_scroll_event(Some(pid), 0.0, 0.0).expect("per-pid scroll post should succeed");
        for button_kind in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
            for event_kind in [
                MouseEventKind::Move,
                MouseEventKind::Down,
                MouseEventKind::Up,
            ] {
                post_mouse_event(
                    Some(pid),
                    Some(window_id),
                    -1.0,
                    -1.0,
                    event_kind,
                    button_kind,
                    0,
                    0.0,
                )
                .expect("per-pid mouse post should succeed");
            }
        }

        assert!(!button.action_names().is_empty());
        button
            .perform_action("AXPress")
            .expect("OK button press should succeed");
    } else {
        let _ = system.element_at_position(probe_point.x, probe_point.y);
        assert!(!set_window_alpha(WindowId(0), 1.0));
        assert_capture_window_result(capture_window(WindowId(0)));
        exercise_observer_api(pid, &app);
        exercise_event_api(pid, None);
    }
}
