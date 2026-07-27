//! Capture + encode pipeline for a booted simulator.
//!
//! Ties [`SimFramebuffer`] to [`H264Encoder`], keeping the encode step on the
//! capture queue so that only compressed bytes ever cross a thread boundary.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use objc2_core_video::CVImageBuffer;

use super::encoder::{ChunkSink, EncoderConfig, H264Encoder};
use super::framebuffer::{FramebufferStats, SimFramebuffer};
use super::recorder::{Recorder, Recording, RecordingConfig};

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
    /// Set while a recording is running. Shared with the capture queue, which
    /// appends to it, so starting and stopping is just swapping this slot.
    recorder: Arc<Mutex<Option<Recorder>>>,
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
        let recorder: Arc<Mutex<Option<Recorder>>> = Arc::new(Mutex::new(None));
        let recorder_for_sink = Arc::clone(&recorder);

        framebuffer.set_sink(Some(Box::new(move |frame| {
            // `CVPixelBuffer` derefs to `CVImageBuffer`, which is what
            // VideoToolbox wants.
            let image: &CVImageBuffer = frame.pixel_buffer;
            if let Err(error) = encoder.encode(image, frame.width as i32, frame.height as i32) {
                eprintln!("[capture] encode failed: {error}");
            }

            // A recording is a second, independent encode of the same frame,
            // which is what lets it use B-frames and its own resolution
            // without disturbing whatever the live viewer is watching.
            if let Ok(mut recorder) = recorder_for_sink.lock()
                && let Some(active) = recorder.as_mut()
                && let Err(error) = active.append(frame.pixel_buffer)
            {
                eprintln!("[record] dropping frame: {error}");
            }
        })));

        framebuffer.start()?;
        Ok(Self {
            framebuffer,
            force_keyframe,
            config,
            recorder,
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

    /// Begin recording to `path`. Fails if one is already running.
    pub fn start_recording(&self, path: &Path, config: RecordingConfig) -> Result<()> {
        let geometry = self.geometry();
        if geometry.width == 0 || geometry.height == 0 {
            anyhow::bail!("no frames captured yet; cannot size a recording");
        }

        let mut slot = self.recorder.lock().unwrap();
        if slot.is_some() {
            anyhow::bail!("a recording is already in progress");
        }
        *slot = Some(Recorder::start(
            path,
            geometry.width,
            geometry.height,
            config,
        )?);
        Ok(())
    }

    /// Stop the running recording and finalize the file.
    pub fn stop_recording(&self) -> Result<Recording> {
        let recorder = self
            .recorder
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| anyhow::anyhow!("no recording is in progress"))?;
        // Finalizing blocks on the writer flushing, so it happens after the
        // slot is cleared and the capture queue has stopped appending.
        recorder.finish()
    }

    pub fn recording_frames(&self) -> Option<u64> {
        self.recorder
            .lock()
            .unwrap()
            .as_ref()
            .map(|recorder| recorder.frames())
    }

    pub fn stop(&mut self) {
        // Finalize before tearing the capture down, so an in-flight recording
        // is left playable rather than truncated.
        if self.recorder.lock().unwrap().is_some() {
            let _ = self.stop_recording();
        }
        self.framebuffer.stop();
    }
}
