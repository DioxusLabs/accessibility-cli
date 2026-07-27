//! Preparing captured frames for the encoder.
//!
//! Three things have to happen between SimulatorKit's framebuffer and
//! VideoToolbox, and they are all the same operation:
//!
//! 1. **Copy.** SimulatorKit recycles its framebuffer `IOSurface` in place, and
//!    VideoToolbox encodes asynchronously, so the encoder must not be reading
//!    the live surface.
//! 2. **Convert.** The framebuffer is BGRA; H.264 encoders want NV12. Handing
//!    BGRA to `VTCompressionSession` makes it convert internally anyway.
//! 3. **Scale.** A phone framebuffer is far larger than anyone views it at.
//!
//! `VTPixelTransferSession` does all three in one hardware pass into a pooled
//! buffer, which is why there is no CPU copy here any more. The previous
//! implementation did a row-wise `memcpy` and left conversion and scaling to
//! the encoder.
//!
//! Note this does not contradict the earlier finding that a plain Metal blit
//! was slower than `memcpy`: that measured a bare copy doing one job against a
//! purpose-built transfer doing three.

use std::ptr::NonNull;

use anyhow::{Result, anyhow};
use objc2_core_foundation::{
    CFDictionary, CFNumber, CFRetained, CFString, CFType, kCFAllocatorDefault,
};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferPool, kCVPixelBufferHeightKey,
    kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
};
use objc2_video_toolbox::VTPixelTransferSession;

/// Build an untyped `CFDictionary` from `CFString` keys to arbitrary CF values.
pub(super) fn cf_dict(pairs: &[(&CFString, &CFType)]) -> CFRetained<CFDictionary> {
    let keys: Vec<&CFString> = pairs.iter().map(|(k, _)| *k).collect();
    let values: Vec<&CFType> = pairs.iter().map(|(_, v)| *v).collect();
    let typed = CFDictionary::<CFString, CFType>::from_slices(&keys, &values);
    // The generic parameters are purely a Rust-side convenience; the underlying
    // CF object is the same either way.
    unsafe { CFRetained::cast_unchecked::<CFDictionary>(typed) }
}

/// Single-entry dictionary mapping a key to a `u32`, as CoreVideo expects.
pub(super) fn cf_dict_u32(key: &CFString, value: u32) -> CFRetained<CFDictionary> {
    let number = CFNumber::new_i32(value as i32);
    cf_dict(&[(key, number.as_ref())])
}

/// Copies, converts and scales frames on the way to the encoder.
pub(super) struct PixelTransfer {
    session: CFRetained<VTPixelTransferSession>,
    pool: Option<CFRetained<CVPixelBufferPool>>,
    /// Output size the current pool was built for.
    dimensions: (usize, usize),
}

// The session and pool are only touched behind the capture state's mutex,
// which serializes the capture queue against the idle timer.
unsafe impl Send for PixelTransfer {}

impl PixelTransfer {
    pub(super) fn new() -> Result<Self> {
        let mut out: *mut VTPixelTransferSession = std::ptr::null_mut();
        let status =
            unsafe { VTPixelTransferSession::create(kCFAllocatorDefault, NonNull::from(&mut out)) };
        let session = NonNull::new(out)
            .filter(|_| status == 0)
            .map(|p| unsafe { CFRetained::from_raw(p) })
            .ok_or_else(|| anyhow!("VTPixelTransferSessionCreate failed: {status}"))?;

        Ok(Self {
            session,
            pool: None,
            dimensions: (0, 0),
        })
    }

    /// Produce an NV12 copy of `source` at the requested size.
    ///
    /// The returned buffer is independent of the source, so the caller may
    /// hand it to an asynchronous encoder.
    pub(super) fn transfer(
        &mut self,
        source: &CVPixelBuffer,
        width: usize,
        height: usize,
    ) -> Result<CFRetained<CVPixelBuffer>> {
        if width == 0 || height == 0 {
            return Err(anyhow!("Target size is empty"));
        }

        if self.pool.is_none() || self.dimensions != (width, height) {
            self.pool = Some(build_pool(width, height)?);
            self.dimensions = (width, height);
        }
        let pool = self.pool.as_ref().expect("pool built above");

        let mut out: *mut CVPixelBuffer = std::ptr::null_mut();
        let status = unsafe {
            CVPixelBufferPool::create_pixel_buffer(
                kCFAllocatorDefault,
                pool,
                NonNull::from(&mut out),
            )
        };
        let destination = NonNull::new(out)
            .filter(|_| status == 0)
            .map(|p| unsafe { CFRetained::from_raw(p) })
            .ok_or_else(|| anyhow!("CVPixelBufferPoolCreatePixelBuffer failed: {status}"))?;

        let status = unsafe { self.session.transfer_image(source, &destination) };
        if status != 0 {
            return Err(anyhow!(
                "VTPixelTransferSessionTransferImage failed: {status}"
            ));
        }
        Ok(destination)
    }
}

/// Create an IOSurface-backed NV12 pool at the given size.
fn build_pool(width: usize, height: usize) -> Result<CFRetained<CVPixelBufferPool>> {
    let format = CFNumber::new_i32(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange as i32);
    let width_num = CFNumber::new_i32(width as i32);
    let height_num = CFNumber::new_i32(height as i32);
    // An empty IOSurface-properties dictionary is the documented way to ask for
    // IOSurface-backed buffers, which keeps VideoToolbox on the zero-copy path.
    let io_surface_props = CFDictionary::<CFString, CFType>::empty();

    let attrs = unsafe {
        cf_dict(&[
            (kCVPixelBufferPixelFormatTypeKey, format.as_ref()),
            (kCVPixelBufferWidthKey, width_num.as_ref()),
            (kCVPixelBufferHeightKey, height_num.as_ref()),
            (
                kCVPixelBufferIOSurfacePropertiesKey,
                &*CFRetained::cast_unchecked::<CFType>(io_surface_props),
            ),
        ])
    };

    let mut out: *mut CVPixelBufferPool = std::ptr::null_mut();
    let status = unsafe {
        CVPixelBufferPool::create(
            kCFAllocatorDefault,
            None,
            Some(&attrs),
            NonNull::from(&mut out),
        )
    };
    NonNull::new(out)
        .filter(|_| status == 0)
        .map(|p| unsafe { CFRetained::from_raw(p) })
        .ok_or_else(|| anyhow!("CVPixelBufferPoolCreate failed: {status}"))
}
