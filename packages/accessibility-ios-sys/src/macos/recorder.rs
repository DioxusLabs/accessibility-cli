//! Recording the simulator screen to an MP4.
//!
//! Deliberately a *second* encoder rather than a retuning of the streaming
//! one. The live path cannot use B-frames — WebRTC's payloader and the raw
//! stream framing both assume the encoder emits frames in the order they were
//! submitted — but a recording has no such constraint and B-frames are where
//! most of the quality-per-bit comes from. Running a separate encode is also
//! what lets a recording be a different resolution and quality from whatever
//! the viewer happens to be watching.
//!
//! `AVAssetWriter` does the encoding and the muxing. Handing it pixel buffers
//! rather than encoding ourselves and appending samples avoids having to order
//! decode and presentation timestamps by hand, which is exactly the part that
//! B-frames complicate.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_av_foundation::{
    AVAssetWriter, AVAssetWriterInput, AVAssetWriterInputPixelBufferAdaptor, AVFileTypeMPEG4,
    AVMediaTypeVideo, AVVideoAllowFrameReorderingKey, AVVideoCodecKey, AVVideoCodecTypeH264,
    AVVideoCompressionPropertiesKey, AVVideoHeightKey, AVVideoMaxKeyFrameIntervalKey,
    AVVideoProfileLevelKey, AVVideoQualityKey, AVVideoWidthKey,
};
use objc2_core_media::CMTime;
use objc2_core_video::CVPixelBuffer;
use objc2_foundation::{NSMutableDictionary, NSNumber, NSString, NSURL};

use super::pixel_buffer::PixelTransfer;

/// Timescale for presentation timestamps. Nanoseconds, so wall-clock elapsed
/// time converts exactly and variable frame rates need no rounding.
const TIMESCALE: i32 = 1_000_000_000;

#[derive(Debug, Clone, Copy)]
pub struct RecordingConfig {
    /// Quality from 0 to 1.
    pub quality: f64,
    /// Longest edge of the recording. `None` records at capture resolution.
    pub max_dimension: Option<u32>,
    /// Seconds between keyframes. Longer is smaller, at the cost of seek
    /// granularity, and a recording is not being seeked while it is written.
    pub keyframe_interval_secs: u32,
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            quality: 0.8,
            // Recordings are watched, not previewed in a corner, so they get
            // more pixels than the live stream's 1280.
            max_dimension: Some(1920),
            keyframe_interval_secs: 4,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Recording {
    pub path: PathBuf,
    pub frames: u64,
    pub duration: Duration,
    pub width: u32,
    pub height: u32,
}

pub struct Recorder {
    writer: Retained<AVAssetWriter>,
    input: Retained<AVAssetWriterInput>,
    adaptor: Retained<AVAssetWriterInputPixelBufferAdaptor>,
    transfer: PixelTransfer,
    path: PathBuf,
    dimensions: (usize, usize),
    frames: AtomicU64,
    /// Set on the first appended frame; timestamps are relative to it.
    started_at: Option<std::time::Instant>,
    last_timestamp: Duration,
}

// Only ever touched behind the capture state's mutex.
unsafe impl Send for Recorder {}

impl Recorder {
    /// Begin a recording of a `source_width` x `source_height` capture.
    pub fn start(
        path: &Path,
        source_width: u32,
        source_height: u32,
        config: RecordingConfig,
    ) -> Result<Self> {
        let (width, height) = recording_size(source_width, source_height, config.max_dimension);

        // AVAssetWriter refuses to overwrite, so clear any previous file.
        let _ = std::fs::remove_file(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let file_type =
            unsafe { AVFileTypeMPEG4 }.ok_or_else(|| anyhow!("AVFileTypeMPEG4 unavailable"))?;
        let writer = unsafe { AVAssetWriter::assetWriterWithURL_fileType_error(&url, file_type) }
            .map_err(|error| anyhow!("AVAssetWriter creation failed: {error}"))?;

        let settings = video_settings(width, height, &config)?;
        let media_type =
            unsafe { AVMediaTypeVideo }.ok_or_else(|| anyhow!("AVMediaTypeVideo unavailable"))?;
        let input = unsafe {
            AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
                media_type,
                Some(&settings),
            )
        };
        // The simulator paints when it feels like it, so frames arrive in real
        // time and the writer should not wait for a full pipeline.
        unsafe { input.setExpectsMediaDataInRealTime(true) };

        let adaptor = unsafe {
            AVAssetWriterInputPixelBufferAdaptor::
                assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(
                    &input, None,
                )
        };

        if !unsafe { writer.canAddInput(&input) } {
            return Err(anyhow!("AVAssetWriter rejected the video input"));
        }
        unsafe { writer.addInput(&input) };

        if !unsafe { writer.startWriting() } {
            let error = unsafe { writer.error() };
            return Err(anyhow!(
                "AVAssetWriter failed to start: {}",
                error
                    .map(|e| e.localizedDescription().to_string())
                    .unwrap_or_else(|| "unknown".into())
            ));
        }
        unsafe { writer.startSessionAtSourceTime(CMTime::new(0, TIMESCALE)) };

        Ok(Self {
            writer,
            input,
            adaptor,
            transfer: PixelTransfer::new()?,
            path: path.to_path_buf(),
            dimensions: (width as usize, height as usize),
            frames: AtomicU64::new(0),
            started_at: None,
            last_timestamp: Duration::ZERO,
        })
    }

    /// Append a captured frame.
    ///
    /// Frames the writer is not ready for are dropped rather than queued: a
    /// recording that falls behind should lose frames, not unbounded memory.
    /// Pixel transfer and writer submission run synchronously on the caller.
    pub fn append(&mut self, source: &CVPixelBuffer) -> Result<()> {
        let started_at = *self.started_at.get_or_insert_with(std::time::Instant::now);

        if !unsafe { self.input.isReadyForMoreMediaData() } {
            return Ok(());
        }

        let (width, height) = self.dimensions;
        let prepared = self.transfer.transfer(source, width, height)?;

        // Wall-clock elapsed, so a variable capture rate is recorded at the
        // speed it actually happened rather than being played back wrong.
        let elapsed = started_at.elapsed();
        let timestamp = unsafe { CMTime::new(elapsed.as_nanos() as i64, TIMESCALE) };
        self.last_timestamp = elapsed;

        let appended = unsafe {
            self.adaptor
                .appendPixelBuffer_withPresentationTime(&prepared, timestamp)
        };
        if !appended {
            let error = unsafe { self.writer.error() };
            return Err(anyhow!(
                "appending a frame failed: {}",
                error
                    .map(|e| e.localizedDescription().to_string())
                    .unwrap_or_else(|| "unknown".into())
            ));
        }
        self.frames.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    /// Finish writing and return the completed file.
    ///
    /// Blocks until the writer has flushed; an MP4 is unplayable without its
    /// trailing index, so returning early would hand back a broken file.
    pub fn finish(self) -> Result<Recording> {
        unsafe { self.input.markAsFinished() };

        let done = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let signal = std::sync::Arc::clone(&done);
        let handler = block2::RcBlock::new(move || {
            let (lock, notify) = &*signal;
            *lock.lock().unwrap() = true;
            notify.notify_all();
        });
        unsafe { self.writer.finishWritingWithCompletionHandler(&handler) };

        let (lock, notify) = &*done;
        let mut finished = lock.lock().unwrap();
        while !*finished {
            let (guard, timeout) = notify
                .wait_timeout(finished, Duration::from_secs(30))
                .expect("recording completion mutex poisoned");
            finished = guard;
            if timeout.timed_out() {
                return Err(anyhow!("timed out waiting for the recording to finalize"));
            }
        }

        if let Some(error) = unsafe { self.writer.error() } {
            return Err(anyhow!(
                "recording failed: {}",
                error.localizedDescription()
            ));
        }

        let metadata = std::fs::metadata(&self.path)
            .with_context(|| format!("recording file missing at {}", self.path.display()))?;
        if metadata.len() == 0 {
            return Err(anyhow!("recording produced an empty file"));
        }

        Ok(Recording {
            path: self.path,
            frames: self.frames.load(Ordering::Relaxed),
            duration: self.last_timestamp,
            width: self.dimensions.0 as u32,
            height: self.dimensions.1 as u32,
        })
    }
}

/// Recording dimensions, preserving aspect and rounded to even numbers.
fn recording_size(width: u32, height: u32, max_dimension: Option<u32>) -> (u32, u32) {
    let longest = width.max(height);
    let scale = match max_dimension {
        Some(limit) if longest > limit => limit as f64 / longest as f64,
        _ => 1.0,
    };
    let even = |value: f64| ((value.round() as u32).max(2) / 2) * 2;
    (even(width as f64 * scale), even(height as f64 * scale))
}

fn video_settings(
    width: u32,
    height: u32,
    config: &RecordingConfig,
) -> Result<Retained<NSMutableDictionary<NSString, AnyObject>>> {
    let compression: Retained<NSMutableDictionary<NSString, AnyObject>> =
        NSMutableDictionary::new();

    // The whole reason recording has its own encoder: B-frames buy real
    // quality per bit and are unusable on the live path, where every consumer
    // assumes output order matches input order.
    put(
        &compression,
        unsafe { AVVideoAllowFrameReorderingKey },
        NSNumber::new_bool(true).as_ref(),
    )?;
    put(
        &compression,
        unsafe { AVVideoQualityKey },
        NSNumber::new_f64(config.quality.clamp(0.0, 1.0)).as_ref(),
    )?;
    put(
        &compression,
        unsafe { AVVideoMaxKeyFrameIntervalKey },
        NSNumber::new_i32((config.keyframe_interval_secs * 60) as i32).as_ref(),
    )?;
    // High profile is worth taking here. The browser-compatibility reasoning
    // that pins the live stream to Baseline does not apply to a file.
    put(
        &compression,
        unsafe { AVVideoProfileLevelKey },
        NSString::from_str("H264_High_AutoLevel").as_ref(),
    )?;

    let settings: Retained<NSMutableDictionary<NSString, AnyObject>> = NSMutableDictionary::new();
    let codec =
        unsafe { AVVideoCodecTypeH264 }.ok_or_else(|| anyhow!("H.264 codec unavailable"))?;
    put(&settings, unsafe { AVVideoCodecKey }, codec.as_ref())?;
    put(
        &settings,
        unsafe { AVVideoWidthKey },
        NSNumber::new_u32(width).as_ref(),
    )?;
    put(
        &settings,
        unsafe { AVVideoHeightKey },
        NSNumber::new_u32(height).as_ref(),
    )?;
    put(
        &settings,
        unsafe { AVVideoCompressionPropertiesKey },
        compression.as_ref(),
    )?;

    Ok(settings)
}

/// Insert into an AVFoundation settings dictionary.
///
/// The keys are optional statics because the framework may not vend them, so
/// every one is checked rather than unwrapped.
fn put(
    dictionary: &NSMutableDictionary<NSString, AnyObject>,
    key: Option<&'static NSString>,
    value: &AnyObject,
) -> Result<()> {
    let key = key.ok_or_else(|| anyhow!("an AVFoundation video settings key was unavailable"))?;
    unsafe { dictionary.setObject_forKey(value, ProtocolObject::from_ref(key)) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_size_preserves_aspect_and_parity() {
        // A phone framebuffer capped at 1920 on its long edge.
        let (width, height) = recording_size(1206, 2622, Some(1920));
        assert_eq!(height, 1920);
        // 1206 * 1920/2622 is 883.1, rounded down to the nearest even number.
        assert_eq!(width, 882);
        assert_eq!(width % 2, 0, "H.264 chroma needs even dimensions");
        assert_eq!(height % 2, 0);
        let aspect_error = ((width as f64 / height as f64) - (1206.0 / 2622.0)).abs();
        assert!(aspect_error < 0.002, "aspect drifted by {aspect_error}");
    }

    #[test]
    fn recording_size_does_not_upscale() {
        assert_eq!(recording_size(588, 1280, Some(1920)), (588, 1280));
    }

    #[test]
    fn recording_size_unbounded_keeps_native_resolution() {
        assert_eq!(recording_size(1206, 2622, None), (1206, 2622));
    }
}
