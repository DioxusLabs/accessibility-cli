use crate::error::{CliError, CliResult};
use accessibility_core::accessibility::{
    AccessibilityEvent, AccessibilityEventType, ListenerConfig, TargetedAccessibility,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub async fn listen(
    adapter: &mut TargetedAccessibility,
    filters: Option<&[String]>,
) -> CliResult<()> {
    if !adapter.supports_event_listening() {
        return Err(CliError::runtime(format!(
            "Event listening is not supported on {}",
            adapter.platform_name()
        )));
    }

    let mut config = ListenerConfig::new().with_buffer_size(256);
    if let Some(filter_strs) = filters {
        let event_types = filter_strs
            .iter()
            .filter_map(|s| {
                let parsed = parse_event_type(s);
                if parsed.is_none() {
                    eprintln!("Warning: Unknown event type '{}', ignoring", s);
                }
                parsed
            })
            .collect::<Vec<_>>();

        if !event_types.is_empty() {
            config = config.with_event_types(event_types.clone());
            println!("Filtering events: {:?}", event_types);
        }
    }

    println!(
        "Starting accessibility event listener on {}...",
        adapter.platform_name()
    );
    println!("Press Ctrl+C to stop.\n");

    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();
    ctrlc::set_handler(move || {
        println!("\nStopping event listener...");
        running_clone.store(false, Ordering::SeqCst);
    })
    .map_err(|e| CliError::runtime(format!("Failed to set Ctrl+C handler: {e}")))?;

    let event_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let event_count_clone = event_count.clone();
    let running_for_callback = running.clone();

    let handle = adapter
        .start_listening(
            config,
            Box::new(move |event| {
                if !running_for_callback.load(Ordering::SeqCst) {
                    return;
                }
                let count = event_count_clone.fetch_add(1, Ordering::SeqCst) + 1;
                print_event(count, &event);
            }),
        )
        .map_err(|e| CliError::runtime(format!("Failed to start event listener: {e}")))?;

    while running.load(Ordering::SeqCst) && handle.is_running() {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    handle.stop().await;

    let total = event_count.load(Ordering::SeqCst);
    println!("\nEvent listener stopped. Total events received: {total}");
    Ok(())
}

fn parse_event_type(s: &str) -> Option<AccessibilityEventType> {
    match s.to_lowercase().as_str() {
        "focus" | "focus-changed" => Some(AccessibilityEventType::FocusChanged),
        "value" | "value-changed" => Some(AccessibilityEventType::ValueChanged),
        "title" | "title-changed" => Some(AccessibilityEventType::TitleChanged),
        "structure" | "structure-changed" => Some(AccessibilityEventType::StructureChanged),
        "window-created" => Some(AccessibilityEventType::WindowCreated),
        "window-destroyed" => Some(AccessibilityEventType::WindowDestroyed),
        "window-focus" | "window-focus-changed" => Some(AccessibilityEventType::WindowFocusChanged),
        "text-selected" | "selected-text-changed" => {
            Some(AccessibilityEventType::SelectedTextChanged)
        }
        "element-destroyed" => Some(AccessibilityEventType::ElementDestroyed),
        _ => None,
    }
}

fn print_event(count: u64, event: &AccessibilityEvent) {
    match event {
        AccessibilityEvent::FocusChanged { element, pid, .. } => {
            let elem_info = element
                .as_ref()
                .map(|e| format!("[{}] {:?} \"{}\"", e.id, e.role, e.display_label()))
                .unwrap_or_else(|| "None".to_string());
            println!(
                "[{}] FOCUS_CHANGED pid={:?} element={}",
                count, pid, elem_info
            );
            if let Some(elem) = element
                && let Some(bounds) = &elem.bounds
            {
                println!(
                    "       bounds: ({:.0}, {:.0}) {}x{}",
                    bounds.origin.x, bounds.origin.y, bounds.size.width, bounds.size.height
                );
            }
        }
        AccessibilityEvent::ValueChanged {
            element,
            old_value,
            new_value,
            ..
        } => {
            let elem_info = element
                .as_ref()
                .map(|e| format!("[{}] {:?} \"{}\"", e.id, e.role, e.display_label()))
                .unwrap_or_else(|| "None".to_string());
            println!("[{}] VALUE_CHANGED element={}", count, elem_info);
            if old_value.is_some() || new_value.is_some() {
                println!(
                    "       old=\"{}\" new=\"{}\"",
                    old_value.as_deref().unwrap_or(""),
                    new_value.as_deref().unwrap_or("")
                );
            }
        }
        AccessibilityEvent::TitleChanged {
            element,
            old_title,
            new_title,
            ..
        } => {
            let elem_info = element
                .as_ref()
                .map(|e| format!("[{}] {:?}", e.id, e.role))
                .unwrap_or_else(|| "None".to_string());
            println!("[{}] TITLE_CHANGED element={}", count, elem_info);
            println!(
                "       old=\"{}\" new=\"{}\"",
                old_title.as_deref().unwrap_or(""),
                new_title.as_deref().unwrap_or("")
            );
        }
        AccessibilityEvent::StructureChanged {
            parent_element,
            change_type,
            ..
        } => {
            let parent_info = parent_element
                .as_ref()
                .map(|e| format!("[{}] {:?} \"{}\"", e.id, e.role, e.display_label()))
                .unwrap_or_else(|| "None".to_string());
            println!(
                "[{}] STRUCTURE_CHANGED type={:?} parent={}",
                count, change_type, parent_info
            );
        }
        AccessibilityEvent::WindowCreated { element, pid, .. } => {
            let elem_info = element
                .as_ref()
                .map(|e| format!("[{}] {:?} \"{}\"", e.id, e.role, e.display_label()))
                .unwrap_or_else(|| "None".to_string());
            println!(
                "[{}] WINDOW_CREATED pid={:?} element={}",
                count, pid, elem_info
            );
        }
        AccessibilityEvent::WindowDestroyed { window_id, pid, .. } => {
            println!(
                "[{}] WINDOW_DESTROYED pid={:?} window_id={:?}",
                count, pid, window_id
            );
        }
        AccessibilityEvent::WindowFocusChanged { element, pid, .. } => {
            let elem_info = element
                .as_ref()
                .map(|e| format!("[{}] {:?} \"{}\"", e.id, e.role, e.display_label()))
                .unwrap_or_else(|| "None".to_string());
            println!(
                "[{}] WINDOW_FOCUS_CHANGED pid={:?} element={}",
                count, pid, elem_info
            );
        }
        AccessibilityEvent::SelectedTextChanged {
            element,
            selected_text,
            ..
        } => {
            let elem_info = element
                .as_ref()
                .map(|e| format!("[{}] {:?}", e.id, e.role))
                .unwrap_or_else(|| "None".to_string());
            println!(
                "[{}] SELECTED_TEXT_CHANGED element={} text=\"{}\"",
                count,
                elem_info,
                selected_text.as_deref().unwrap_or("")
            );
        }
        AccessibilityEvent::ElementDestroyed { element_id, .. } => {
            println!("[{}] ELEMENT_DESTROYED id={:?}", count, element_id);
        }
        AccessibilityEvent::Error { message, .. } => {
            eprintln!("[{}] ERROR: {}", count, message);
        }
        AccessibilityEvent::Stopped { reason, .. } => {
            println!("[{}] STOPPED reason={:?}", count, reason);
        }
    }
}
