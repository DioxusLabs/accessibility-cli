//! Pooled deep copies of live framebuffer pixel buffers.
//!
//! SimulatorKit hands out a `CVPixelBuffer` that wraps its own framebuffer
//! `IOSurface`, and it recycles that surface in place. Retaining the pixel
//! buffer does not help, because the *surface* mutates underneath it. Since
//! VideoToolbox encodes asynchronously, anything downstream must own its own
//! copy or it will encode torn frames.
//!
//! The copy is a straightforward row-wise `memcpy` into a size-keyed
//! `CVPixelBufferPool`.
//!
//! This looks like an obvious target for a GPU blit, and it isn't. Both
//! surfaces are IOSurface-backed, so they can be wrapped as `MTLTexture`s and
//! copied with a blit encoder — but that was measured at **0.517 ms/frame
//! against 0.377 ms for the `memcpy`** on an iPhone 17 surface (1206x2622,
//! 12.1 MB), with cache-cold sources. Command buffer submission plus the
//! `waitUntilCompleted` round trip costs more than the copy saves, because
//! unified memory already gives the CPU path ~34 GB/s.
//!
//! At 60fps the `memcpy` is ~23 ms/s, or about 2% of one core. It is not worth
//! optimizing, and the GPU version was slower and more complex. Measure before
//! trying again.

use std::ptr::NonNull;

use anyhow::{Result, anyhow};
use objc2_core_foundation::{
    CFDictionary, CFNumber, CFRetained, CFString, CFType, kCFAllocatorDefault,
};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferPool, CVPixelBufferUnlockBaseAddress,
    kCVPixelBufferHeightKey, kCVPixelBufferIOSurfacePropertiesKey,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey, kCVPixelFormatType_32BGRA,
};

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

/// Reusable pool of BGRA pixel buffers, rebuilt whenever the source resizes.
pub(super) struct Photocopier {
    pool: Option<CFRetained<CVPixelBufferPool>>,
    dimensions: (usize, usize),
}

// The pool is only ever touched behind the capture state's mutex, which
// serializes the capture queue against the idle timer.
unsafe impl Send for Photocopier {}

impl Photocopier {
    pub(super) fn new() -> Self {
        Self {
            pool: None,
            dimensions: (0, 0),
        }
    }

    /// Deep-copy `source` into a pooled buffer of the same size.
    pub(super) fn copy(&mut self, source: &CVPixelBuffer) -> Result<CFRetained<CVPixelBuffer>> {
        let width = CVPixelBufferGetWidth(source);
        let height = CVPixelBufferGetHeight(source);
        if width == 0 || height == 0 {
            return Err(anyhow!("Source pixel buffer has zero extent"));
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

        unsafe {
            CVPixelBufferLockBaseAddress(source, CVPixelBufferLockFlags::ReadOnly);
            CVPixelBufferLockBaseAddress(&destination, CVPixelBufferLockFlags::empty());
        }

        let result = (|| {
            let src_base = CVPixelBufferGetBaseAddress(source);
            let dst_base = CVPixelBufferGetBaseAddress(&destination);
            if src_base.is_null() || dst_base.is_null() {
                return Err(anyhow!("Pixel buffer base address unavailable"));
            }

            let src_stride = CVPixelBufferGetBytesPerRow(source);
            let dst_stride = CVPixelBufferGetBytesPerRow(&destination);
            let row_bytes = src_stride.min(dst_stride);

            for row in 0..height {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        (src_base as *const u8).add(row * src_stride),
                        (dst_base as *mut u8).add(row * dst_stride),
                        row_bytes,
                    );
                }
            }
            Ok(())
        })();

        unsafe {
            CVPixelBufferUnlockBaseAddress(&destination, CVPixelBufferLockFlags::empty());
            CVPixelBufferUnlockBaseAddress(source, CVPixelBufferLockFlags::ReadOnly);
        }

        result.map(|()| destination)
    }
}

/// Create an IOSurface-backed BGRA pool at the given size.
fn build_pool(width: usize, height: usize) -> Result<CFRetained<CVPixelBufferPool>> {
    let format = CFNumber::new_i32(kCVPixelFormatType_32BGRA as i32);
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
