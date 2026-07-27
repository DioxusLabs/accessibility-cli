//! Capture + encode pipeline for a booted simulator.
//!
//! Ties [`SimFramebuffer`] to [`H264Encoder`], keeping the encode step on the
//! capture queue so that only compressed bytes ever cross a thread boundary.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use objc2_core_video::CVImageBuffer;

use super::encoder::{ChunkSink, EncoderConfig, H264Encoder};
use super::framebuffer::{FramebufferStats, SimFramebuffer};

/// Pixel geometry of the captured display.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScreenGeometry {
    pub width: u32,
    pub height: u32,
}

/// A running capture-and-encode session.
pub struct SimVideoStream {
    framebuffer: SimFramebuffer,
    force_keyframe: Arc<AtomicBool>,
    config: EncoderConfig,
}

impl SimVideoStream {
    /// Start capturing `udid` and pushing encoded chunks to `sink`.
    ///
    /// `sink` runs on the capture queue, so it must not block; the intended
    /// use is a bounded channel that drops on overflow.
    pub fn start(udid: Option<&str>, config: EncoderConfig, sink: ChunkSink) -> Result<Self> {
        let mut framebuffer = SimFramebuffer::new(udid)?;
        let force_keyframe = Arc::new(AtomicBool::new(false));

        let mut encoder = H264Encoder::new(config, Arc::clone(&force_keyframe), sink)?;
        framebuffer.set_sink(Some(Box::new(move |frame| {
            // `CVPixelBuffer` derefs to `CVImageBuffer`, which is what
            // VideoToolbox wants.
            let image: &CVImageBuffer = frame.pixel_buffer;
            if let Err(error) = encoder.encode(image, frame.width as i32, frame.height as i32) {
                eprintln!("[capture] encode failed: {error}");
            }
        })));

        framebuffer.start()?;
        Ok(Self {
            framebuffer,
            force_keyframe,
            config,
        })
    }

    pub fn device_udid(&self) -> &str {
        self.framebuffer.device_udid()
    }

    pub fn stats(&self) -> FramebufferStats {
        self.framebuffer.stats()
    }

    pub fn geometry(&self) -> ScreenGeometry {
        let stats = self.framebuffer.stats();
        ScreenGeometry {
            width: stats.width,
            height: stats.height,
        }
    }

    /// Geometry actually handed to the encoder, after any downscale.
    pub fn encoded_geometry(&self) -> ScreenGeometry {
        let source = self.geometry();
        if source.width == 0 || source.height == 0 {
            return source;
        }
        let (width, height) = self
            .config
            .encode_size(source.width as i32, source.height as i32);
        ScreenGeometry {
            width: width as u32,
            height: height as u32,
        }
    }

    /// Ask the encoder to make the next frame an IDR.
    ///
    /// Driven by RTCP PLI/FIR from WebRTC receivers, and by new subscribers on
    /// the raw stream endpoints.
    pub fn request_keyframe(&self) {
        self.force_keyframe.store(true, Ordering::Relaxed);
    }

    pub fn stop(&mut self) {
        self.framebuffer.stop();
    }
}
