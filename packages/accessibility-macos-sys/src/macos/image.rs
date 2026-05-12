use super::PngImage;
use anyhow::{Result, anyhow, bail};
use objc2::{AnyThread, runtime::AnyObject};
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSBitmapImageRepPropertyKey};
use objc2_core_graphics::CGImage;
use objc2_foundation::NSDictionary;
use std::ffi::c_void;
use std::ptr::NonNull;

pub(crate) fn encode_cg_image_as_png(image: &CGImage) -> Result<PngImage> {
    let width = CGImage::width(Some(image)) as u32;
    let height = CGImage::height(Some(image)) as u32;
    if width == 0 || height == 0 {
        bail!("Captured image has empty dimensions: {}x{}", width, height);
    }

    let bitmap = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), image);
    let properties = NSDictionary::<NSBitmapImageRepPropertyKey, AnyObject>::new();
    let data = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties)
    }
    .ok_or_else(|| anyhow!("Failed to encode screenshot as PNG"))?;

    let len = data.length();
    if len == 0 {
        bail!("Encoded screenshot is empty");
    }

    let mut bytes = vec![0; len];
    unsafe {
        data.getBytes_length(
            NonNull::new(bytes.as_mut_ptr().cast::<c_void>())
                .expect("Vec pointer should be non-null"),
            len,
        );
    }

    Ok(PngImage {
        data: bytes,
        width,
        height,
    })
}
