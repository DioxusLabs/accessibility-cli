use crate::cli::{ScreenshotAnnotateCommand, ScreenshotOverlayArgs};
use crate::error::{CliError, CliResult};
use crate::operations::tree::element_overlaps_bounds;
use accessibility_core::accessibility::{ElementTree, TargetedAccessibility};
use accessibility_core::api::{
    annotate_elements, decode_screenshot, draw_grid_overlay, format_role_short, truncate,
};

pub async fn screen(
    adapter: &TargetedAccessibility,
    overlay: &ScreenshotOverlayArgs,
) -> CliResult<()> {
    println!("Capturing full screen screenshot...");
    let screenshot = adapter
        .capture_screen()
        .map_err(|e| CliError::runtime(format!("Failed to capture screen: {e}")))?;

    if overlay.overlay {
        match adapter.get_screen_bounds().await {
            Ok(bounds) => {
                let mut img = decode_screenshot(&screenshot);
                draw_grid_overlay(
                    &mut img,
                    overlay.grid_size,
                    &bounds,
                    screenshot.width,
                    screenshot.height,
                );
                let filename = screenshot_path();
                img.save(&filename).map_err(|e| {
                    CliError::runtime(format!(
                        "Failed to save image to {}: {e}",
                        filename.display()
                    ))
                })?;
                println!(
                    "Saved overlay screenshot to {} ({}x{} pixels, grid size: {})",
                    filename.display(),
                    screenshot.width,
                    screenshot.height,
                    overlay.grid_size
                );
            }
            Err(e) => {
                let filename = screenshot_path();
                std::fs::write(&filename, &screenshot.data).map_err(|write_error| {
                    CliError::runtime(format!(
                        "Failed to get screen bounds: {e}\nFailed to save screenshot to {}: {write_error}",
                        filename.display()
                    ))
                })?;
                println!(
                    "Saved screenshot to {} ({}x{})",
                    filename.display(),
                    screenshot.width,
                    screenshot.height
                );
            }
        }
    } else {
        let filename = screenshot_path();
        std::fs::write(&filename, &screenshot.data).map_err(|e| {
            CliError::runtime(format!("Failed to write {}: {e}", filename.display()))
        })?;
        println!(
            "Saved full screen screenshot to {} ({}x{} pixels)",
            filename.display(),
            screenshot.width,
            screenshot.height
        );
    }

    Ok(())
}

pub async fn elements(
    adapter: &TargetedAccessibility,
    tree: &ElementTree,
    selector: Option<&str>,
) -> CliResult<()> {
    let (elements, description) = adapter
        .find_elements(tree, selector, true)
        .map(|elements| {
            (
                elements,
                if selector.is_some() {
                    "matching"
                } else {
                    "interactive"
                },
            )
        })
        .map_err(|e| CliError::runtime(e.to_string()))?;

    let screenshot = adapter
        .capture_screen()
        .map_err(|e| CliError::runtime(format!("Failed to capture screen: {e}")))?;
    let screen_bounds = adapter
        .get_screen_bounds()
        .await
        .map_err(|e| CliError::runtime(format!("Failed to get screen bounds: {e}")))?;

    let candidate_count = elements.len();
    let elements = elements
        .into_iter()
        .filter(|element| element_overlaps_bounds(element, &screen_bounds))
        .collect::<Vec<_>>();

    println!(
        "Found {} {} elements with drawable bounds",
        elements.len(),
        description
    );
    let skipped = candidate_count.saturating_sub(elements.len());
    if skipped > 0 {
        println!("Skipped {skipped} elements outside the capture or with empty bounds");
    }

    for (i, elem) in elements.iter().enumerate() {
        if let Some(bounds) = &elem.bounds {
            match screenshot.crop(bounds, &screen_bounds) {
                Ok(cropped) => {
                    let filename = screenshot_path();
                    std::fs::write(&filename, &cropped.data).map_err(|e| {
                        CliError::runtime(format!("Failed to write {}: {e}", filename.display()))
                    })?;
                    println!(
                        "  [{}] {} {:?} \"{}\" -> {} ({}x{})",
                        i + 1,
                        elem.id,
                        elem.role,
                        truncate(&elem.display_label(), 30),
                        filename.display(),
                        cropped.width,
                        cropped.height
                    );
                }
                Err(e) => eprintln!("  [{}] {} {:?} -> ERROR: {}", i + 1, elem.id, elem.role, e),
            }
        }
    }

    Ok(())
}

pub async fn annotate(
    adapter: &TargetedAccessibility,
    tree: &ElementTree,
    args: &ScreenshotAnnotateCommand,
) -> CliResult<()> {
    let selector = args.selector.as_deref();
    let (elements, description) = adapter
        .find_elements(tree, selector, true)
        .map(|elements| {
            (
                elements,
                if selector.is_some() {
                    "matching"
                } else {
                    "interactive"
                },
            )
        })
        .map_err(|e| CliError::runtime(e.to_string()))?;

    if elements.is_empty() {
        println!("No elements to annotate.");
        return Ok(());
    }

    let screenshot = adapter
        .capture_screen()
        .map_err(|e| CliError::runtime(format!("Failed to capture screenshot: {e}")))?;
    let screen_bounds = adapter
        .get_screen_bounds()
        .await
        .map_err(|e| CliError::runtime(format!("Failed to get screen bounds: {e}")))?;

    let candidate_count = elements.len();
    let elements = elements
        .into_iter()
        .filter(|element| element_overlaps_bounds(element, &screen_bounds))
        .collect::<Vec<_>>();

    if elements.is_empty() {
        println!("No elements to annotate in captured bounds.");
        return Ok(());
    }

    println!(
        "Found {} {} elements with drawable bounds",
        elements.len(),
        description
    );
    let skipped = candidate_count.saturating_sub(elements.len());
    if skipped > 0 {
        println!("Skipped {skipped} elements outside the capture or with empty bounds");
    }

    let mut img = decode_screenshot(&screenshot);
    let marked = annotate_elements(&mut img, &elements, &screen_bounds, &screenshot, args.label);

    if args.overlay.overlay {
        draw_grid_overlay(
            &mut img,
            args.overlay.grid_size,
            &screen_bounds,
            screenshot.width,
            screenshot.height,
        );
    }

    let filename = screenshot_path();
    img.save(&filename).map_err(|e| {
        CliError::runtime(format!(
            "Failed to save annotated image to {}: {e}",
            filename.display()
        ))
    })?;
    println!(
        "Saved annotated screenshot to {} ({} elements marked)",
        filename.display(),
        marked
    );

    if args.label {
        print_labels(
            &elements,
            &screen_bounds,
            screenshot.width,
            screenshot.height,
            img.width(),
            img.height(),
        );
    }

    Ok(())
}

fn print_labels(
    elements: &[&accessibility_core::accessibility::Element],
    screen_bounds: &accessibility_core::accessibility::Rect,
    screenshot_width: u32,
    screenshot_height: u32,
    image_width: u32,
    image_height: u32,
) {
    println!("\nElement labels:");
    let scale_x = screenshot_width as f64 / screen_bounds.size.width;
    let scale_y = screenshot_height as f64 / screen_bounds.size.height;
    let mut label_num = 1u32;
    for elem in elements {
        if let Some(bounds) = &elem.bounds {
            let px = ((bounds.origin.x - screen_bounds.origin.x) * scale_x) as i32;
            let py = ((bounds.origin.y - screen_bounds.origin.y) * scale_y) as i32;
            let pw = (bounds.size.width * scale_x) as i32;
            let ph = (bounds.size.height * scale_y) as i32;
            if pw > 0
                && ph > 0
                && px < image_width as i32
                && py < image_height as i32
                && px.saturating_add(pw) > 0
                && py.saturating_add(ph) > 0
            {
                let role_str = format_role_short(elem.role);
                println!(
                    "  {}: [{}] {} \"{}\"",
                    label_num,
                    elem.id,
                    role_str,
                    truncate(&elem.display_label(), 30)
                );
                label_num += 1;
            }
        }
    }
}

fn screenshot_path() -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u32)
        .unwrap_or(0);
    let random_id = std::process::id() ^ timestamp;
    std::env::temp_dir().join(format!("{:x}.png", random_id))
}
