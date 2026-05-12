use super::image::encode_cg_image_as_png;
use super::{PngImage, Point, Rect, Size};
use anyhow::{Result, anyhow};
use objc2_core_foundation::CGRect;
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGDisplayPixelsHigh, CGDisplayPixelsWide, CGError,
    CGGetActiveDisplayList, CGMainDisplayID,
};

pub fn main_display_bounds() -> Rect {
    display_bounds(main_display_id())
}

pub fn capture_main_display() -> Result<PngImage> {
    #[allow(deprecated)]
    let image = objc2_core_graphics::CGDisplayCreateImage(main_display_id())
        .ok_or_else(|| anyhow!("Failed to capture main display"))?;

    encode_cg_image_as_png(&image)
}

fn main_display_id() -> CGDirectDisplayID {
    let display_id = CGMainDisplayID();
    if display_has_size(display_id) {
        return display_id;
    }

    let mut displays = [0; 16];
    let mut count = 0u32;
    let result = unsafe {
        CGGetActiveDisplayList(
            displays.len() as u32,
            displays.as_mut_ptr(),
            &mut count as *mut u32,
        )
    };
    if result != CGError::Success {
        return display_id;
    }

    displays
        .into_iter()
        .take(count as usize)
        .find(|display_id| display_has_size(*display_id))
        .unwrap_or(display_id)
}

fn display_bounds(display_id: CGDirectDisplayID) -> Rect {
    let bounds = rect_from_cg_rect(CGDisplayBounds(display_id));
    if bounds.size.width > 0.0 && bounds.size.height > 0.0 {
        return bounds;
    }

    Rect::new(
        bounds.origin,
        Size::new(
            CGDisplayPixelsWide(display_id) as f64,
            CGDisplayPixelsHigh(display_id) as f64,
        ),
    )
}

fn display_has_size(display_id: CGDirectDisplayID) -> bool {
    if display_id == 0 {
        return false;
    }

    let bounds = CGDisplayBounds(display_id);
    (bounds.size.width > 0.0 && bounds.size.height > 0.0)
        || (CGDisplayPixelsWide(display_id) > 0 && CGDisplayPixelsHigh(display_id) > 0)
}

fn rect_from_cg_rect(rect: CGRect) -> Rect {
    Rect::new(
        Point::new(rect.origin.x, rect.origin.y),
        Size::new(rect.size.width, rect.size.height),
    )
}
