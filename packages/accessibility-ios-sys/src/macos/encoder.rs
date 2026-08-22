//! Hardware H.264 encoding of captured simulator frames via VideoToolbox.
//!
//! The encoder is deliberately driven from the capture queue: `CVPixelBuffer`
//! and `CMSampleBuffer` are not `Send`, so encoding in place and shipping only
//! the compressed bytes elsewhere avoids marshalling raw frames across threads.
//!
//! Two output framings are supported from the same session:
//!
//! - [`NalFormat::AnnexB`] for WebRTC, which expects start-code delimited NALs.
//! - [`NalFormat::Avcc`] for browser `VideoDecoder`, which wants length-prefixed
//!   NALs plus a separate `avcC` parameter-set blob.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Result, anyhow};
use block2::RcBlock;
use bytes::Bytes;
use objc2_core_foundation::{
    CFBoolean, CFNumber, CFRetained, CFString, CFType, kCFAllocatorDefault,
};
use objc2_core_media::{
    CMSampleBuffer, CMTime, CMVideoCodecType, CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
    kCMTimeInvalid,
};
use objc2_core_video::{CVImageBuffer, CVPixelBuffer};
use objc2_video_toolbox::{VTCompressionSession, VTEncodeInfoFlags, VTSessionSetProperty};

use super::pixel_buffer::{PixelTransfer, cf_dict};

/// `avc1` — H.264 in a `CMVideoCodecType`.
const CODEC_H264: CMVideoCodecType = 0x6176_6331;

/// How the compressed NAL units are framed on the way out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NalFormat {
    /// `00 00 00 01` start codes, with SPS/PPS inlined ahead of each IDR.
    AnnexB,
    /// 4-byte big-endian length prefixes, exactly as VideoToolbox emits them.
    Avcc,
}

/// Kind of payload in an [`EncodedChunk`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkKind {
    /// An `avcC` parameter set record. Only emitted in [`NalFormat::Avcc`].
    ParameterSet,
    /// An IDR access unit.
    Keyframe,
    /// A non-IDR access unit.
    Delta,
}

/// One encoded unit ready for the wire.
#[derive(Debug, Clone)]
pub struct EncodedChunk {
    pub data: Bytes,
    pub kind: ChunkKind,
    pub captured_at: Instant,
}

/// Bits per pixel per frame to aim for when no explicit bitrate is given.
///
/// Screen content needs roughly 0.10-0.20 bpp to avoid visible blocking on
/// motion. Below that VideoToolbox does not merely soften the picture: with
/// low-latency rate control it starts *dropping frames* to stay inside its
/// per-frame budget, so starving the encoder costs frame rate as well as
/// quality.
const TARGET_BITS_PER_PIXEL: f64 = 0.15;

/// Longest edge to encode at when no limit is given.
///
/// A phone framebuffer is far larger than the browser ever displays it — an
/// iPhone 17 is 1206x2622, roughly fifteen times the pixels of the preview —
/// and every one of those pixels costs bitrate. Capping the long edge keeps
/// the picture comfortably sharper than the viewport while spending a
/// fraction of the bits.
const DEFAULT_MAX_DIMENSION: u32 = 1280;

/// What the encoder should optimize for.
///
/// These are not independent knobs. Constant-quality rate control is *ignored*
/// when `EnableLowLatencyRateControl` is set — measured at 0.0221 bits per
/// pixel for quality 1.0 against 0.0223 for quality 0.4, i.e. no effect at
/// all. Pairing the two as one choice makes that combination unrepresentable
/// instead of silently doing nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tuning {
    /// Live interactive streaming. Low-latency rate control and no frame
    /// delay, which costs perhaps 300ms of decoder buffering if omitted.
    /// Spends a fixed bitrate; quality varies with how busy the screen is.
    Interactive {
        /// Target bitrate, or `None` to derive one from the encode resolution.
        bitrate: Option<u32>,
    },
    /// Recording and offline capture. Drops the low-latency constraint so the
    /// encoder honours a constant quality target and spends bits where the
    /// picture needs them. Latency is unbounded in principle, so this is the
    /// wrong choice for anything anyone is watching live.
    Recording {
        /// Target quality from 0 to 1.
        quality: f64,
    },
}

impl Tuning {
    fn is_interactive(self) -> bool {
        matches!(self, Tuning::Interactive { .. })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EncoderConfig {
    pub fps: u32,
    pub tuning: Tuning,
    /// Longest edge of the encoded video. The source is scaled down to fit.
    pub max_dimension: Option<u32>,
    /// Seconds between scheduled keyframes.
    pub keyframe_interval_secs: u32,
    pub nal_format: NalFormat,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            fps: 60,
            tuning: Tuning::Interactive { bitrate: None },
            max_dimension: Some(DEFAULT_MAX_DIMENSION),
            keyframe_interval_secs: 2,
            nal_format: NalFormat::AnnexB,
        }
    }
}

impl EncoderConfig {
    /// Encode dimensions for a given source size, preserving aspect ratio.
    ///
    /// Both axes are rounded to even numbers because H.264 chroma is
    /// subsampled 2x2 and odd dimensions are not representable.
    pub fn encode_size(&self, width: i32, height: i32) -> (i32, i32) {
        let longest = width.max(height);
        let Some(limit) = self.max_dimension else {
            return (even(width), even(height));
        };
        if longest <= limit as i32 {
            return (even(width), even(height));
        }
        let scale = limit as f64 / longest as f64;
        (
            even((width as f64 * scale).round() as i32),
            even((height as f64 * scale).round() as i32),
        )
    }

    /// Bitrate for interactive tuning, derived from the encode size if unset.
    fn resolved_bitrate(&self, bitrate: Option<u32>, width: i32, height: i32) -> u32 {
        bitrate.unwrap_or_else(|| {
            let pixels = (width as f64) * (height as f64);
            (pixels * self.fps as f64 * TARGET_BITS_PER_PIXEL) as u32
        })
    }
}

fn even(value: i32) -> i32 {
    (value.max(2) / 2) * 2
}

/// Sink for encoded chunks, invoked on the capture queue.
pub type ChunkSink = Arc<dyn Fn(EncodedChunk) + Send + Sync>;

pub struct H264Encoder {
    session: Option<CFRetained<VTCompressionSession>>,
    config: EncoderConfig,
    /// Size of the frames coming in.
    source_dimensions: (i32, i32),
    /// Size we actually encode at, after any downscale.
    dimensions: (i32, i32),
    /// Frame index, used to synthesize presentation timestamps.
    frame_index: i64,
    /// Set by RTCP PLI/FIR so the next frame is forced to an IDR.
    force_keyframe: Arc<AtomicBool>,
    /// Whether the `avcC` record for the current session has been emitted.
    emitted_parameter_set: Arc<Mutex<bool>>,
    /// Copies, converts and scales each frame before it is encoded.
    transfer: PixelTransfer,
    sink: ChunkSink,
}

// VideoToolbox sessions are documented as thread-safe, and this one is only
// driven from the capture queue regardless.
unsafe impl Send for H264Encoder {}

impl H264Encoder {
    pub fn new(
        config: EncoderConfig,
        force_keyframe: Arc<AtomicBool>,
        sink: ChunkSink,
    ) -> Result<Self> {
        Ok(Self {
            transfer: PixelTransfer::new()?,
            session: None,
            config,
            source_dimensions: (0, 0),
            dimensions: (0, 0),
            frame_index: 0,
            force_keyframe,
            emitted_parameter_set: Arc::new(Mutex::new(false)),
            sink,
        })
    }

    /// Encode one frame, rebuilding the session if the source resized.
    ///
    /// `width`/`height` describe the *source*; the session may be smaller if
    /// the config caps the long edge, in which case VideoToolbox scales. The
    /// pixel transfer is synchronous and blocks until the encoder owns a copy
    /// of the recycled simulator surface.
    pub fn encode(
        &mut self,
        image: &CVImageBuffer,
        width: i32,
        height: i32,
        captured_at: Instant,
    ) -> Result<()> {
        if self.session.is_none() || self.source_dimensions != (width, height) {
            self.source_dimensions = (width, height);
            self.dimensions = self.config.encode_size(width, height);
            self.rebuild_session()?;
        }

        // One hardware pass does the copy off the live framebuffer, the BGRA
        // to NV12 conversion, and the downscale. Synchronous, so by the time
        // it returns the encoder owns pixels the simulator cannot overwrite.
        let (target_width, target_height) = self.dimensions;
        let source: &CVPixelBuffer = image;
        let prepared =
            self.transfer
                .transfer(source, target_width as usize, target_height as usize)?;
        let image: &CVImageBuffer = &prepared;

        let session = self.session.as_ref().expect("session built above");

        let force = self.force_keyframe.swap(false, Ordering::Relaxed);
        let frame_properties = force.then(|| {
            cf_dict(&[(
                unsafe { objc2_video_toolbox::kVTEncodeFrameOptionKey_ForceKeyFrame },
                CFBoolean::new(true).as_ref(),
            )])
        });

        let pts = unsafe { CMTime::new(self.frame_index, self.config.fps as i32) };
        let duration = unsafe { CMTime::new(1, self.config.fps as i32) };
        self.frame_index += 1;

        let sink = Arc::clone(&self.sink);
        let nal_format = self.config.nal_format;
        let emitted = Arc::clone(&self.emitted_parameter_set);

        let handler = RcBlock::new(
            move |status: i32, _flags: VTEncodeInfoFlags, sample: *mut CMSampleBuffer| {
                if status != 0 || sample.is_null() {
                    return;
                }
                let sample = unsafe { &*sample };
                for chunk in package_sample(sample, nal_format, &emitted, captured_at) {
                    sink(chunk);
                }
            },
        );

        let status = unsafe {
            session.encode_frame_with_output_handler(
                image,
                pts,
                duration,
                frame_properties.as_deref(),
                std::ptr::null_mut(),
                RcBlock::as_ptr(&handler) as *mut _,
            )
        };
        if status != 0 {
            return Err(anyhow!("VTCompressionSessionEncodeFrame failed: {status}"));
        }
        Ok(())
    }

    fn rebuild_session(&mut self) -> Result<()> {
        if let Some(session) = self.session.take() {
            unsafe { session.invalidate() };
        }
        *self.emitted_parameter_set.lock().unwrap() = false;

        let (width, height) = self.dimensions;
        let mut out: *mut VTCompressionSession = std::ptr::null_mut();

        // Low-latency rate control keeps the decoder's frame buffering small.
        // Without it browsers routinely add ~300ms of latency even though we
        // emit no B-frames. It also makes the encoder ignore any quality
        // target, which is exactly why recording tuning omits it.
        let interactive = self.config.tuning.is_interactive();
        let low_latency = interactive.then(|| {
            cf_dict(&[(
                unsafe {
                    objc2_video_toolbox::kVTVideoEncoderSpecification_EnableLowLatencyRateControl
                },
                CFBoolean::new(true).as_ref(),
            )])
        });

        let mut status = unsafe {
            VTCompressionSession::create(
                kCFAllocatorDefault,
                width,
                height,
                CODEC_H264,
                low_latency.as_deref(),
                None,
                kCFAllocatorDefault,
                None,
                std::ptr::null_mut(),
                NonNull::from(&mut out),
            )
        };
        if status != 0 || out.is_null() {
            out = std::ptr::null_mut();
            status = unsafe {
                VTCompressionSession::create(
                    kCFAllocatorDefault,
                    width,
                    height,
                    CODEC_H264,
                    None,
                    None,
                    kCFAllocatorDefault,
                    None,
                    std::ptr::null_mut(),
                    NonNull::from(&mut out),
                )
            };
        }

        let session = NonNull::new(out)
            .filter(|_| status == 0)
            .map(|p| unsafe { CFRetained::from_raw(p) })
            .ok_or_else(|| anyhow!("VTCompressionSessionCreate failed: {status}"))?;

        let keyframe_interval = self.config.fps * self.config.keyframe_interval_secs;
        set_bool(&session, "RealTime", true);
        // Frames stay in decode order in both tunings. B-frames would help
        // recording quality, but every consumer downstream — WebRTC's
        // payloader and the raw stream's framing — assumes output order
        // matches input order.
        set_bool(&session, "AllowFrameReordering", false);
        if interactive {
            // Emit each frame as soon as it is encoded rather than letting the
            // encoder hold a pipeline of them.
            set_i32(&session, "MaxFrameDelayCount", 0);
        }
        // Baseline is the safest bet for WebRTC: every browser negotiates it,
        // and we gain nothing from High on a UI stream with no B-frames.
        set_string(&session, "ProfileLevel", "H264_Baseline_AutoLevel");
        set_i32(&session, "ExpectedFrameRate", self.config.fps as i32);
        set_i32(&session, "MaxKeyFrameInterval", keyframe_interval as i32);
        // MaxKeyFrameInterval counts *frames*, and the simulator's frame rate
        // swings between ~5 idle and ~60 animating, so on its own it would
        // stretch a "2 second" interval out to twenty. The duration limit is
        // what actually bounds it in time.
        set_f64(
            &session,
            "MaxKeyFrameIntervalDuration",
            self.config.keyframe_interval_secs as f64,
        );
        match self.config.tuning {
            Tuning::Interactive { bitrate } => {
                let bitrate = self.config.resolved_bitrate(bitrate, width, height);
                set_i32(&session, "AverageBitRate", bitrate as i32);
            }
            Tuning::Recording { quality } => {
                set_f64(&session, "Quality", quality.clamp(0.0, 1.0));
            }
        }

        self.session = Some(session);
        self.frame_index = 0;
        Ok(())
    }
}

impl Drop for H264Encoder {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            unsafe {
                session.complete_frames(kCMTimeInvalid);
                session.invalidate();
            }
        }
    }
}

/// Turn one compressed sample into the chunks that go on the wire.
fn package_sample(
    sample: &CMSampleBuffer,
    format: NalFormat,
    emitted_parameter_set: &Mutex<bool>,
    captured_at: Instant,
) -> Vec<EncodedChunk> {
    let is_keyframe = sample_is_keyframe(sample);
    let Some(avcc) = sample_bytes(sample) else {
        return Vec::new();
    };

    let mut chunks = Vec::new();
    let parameter_sets = is_keyframe
        .then(|| unsafe { sample.format_description() })
        .flatten()
        .and_then(|desc| extract_parameter_sets(&desc));

    match format {
        NalFormat::Avcc => {
            // The browser needs the avcC record once per session, before the
            // first keyframe it decodes.
            if let Some((sps, pps)) = parameter_sets.as_ref() {
                let mut emitted = emitted_parameter_set.lock().unwrap();
                if !*emitted {
                    *emitted = true;
                    chunks.push(EncodedChunk {
                        data: Bytes::from(build_avcc_record(sps, pps)),
                        kind: ChunkKind::ParameterSet,
                        captured_at,
                    });
                }
            }
            chunks.push(EncodedChunk {
                data: Bytes::from(avcc),
                kind: if is_keyframe {
                    ChunkKind::Keyframe
                } else {
                    ChunkKind::Delta
                },
                captured_at,
            });
        }
        NalFormat::AnnexB => {
            let mut out = Vec::with_capacity(avcc.len() + 64);
            // SPS/PPS are only prepended to IDRs; repeating them on every
            // delta frame is pure waste.
            if let Some((sps, pps)) = parameter_sets.as_ref() {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(sps);
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(pps);
            }
            append_annex_b(&avcc, &mut out);
            chunks.push(EncodedChunk {
                data: Bytes::from(out),
                kind: if is_keyframe {
                    ChunkKind::Keyframe
                } else {
                    ChunkKind::Delta
                },
                captured_at,
            });
        }
    }

    chunks
}

/// Rewrite AVCC length prefixes as Annex-B start codes.
fn append_annex_b(avcc: &[u8], out: &mut Vec<u8>) {
    let mut offset = 0usize;
    while offset + 4 <= avcc.len() {
        let length = u32::from_be_bytes([
            avcc[offset],
            avcc[offset + 1],
            avcc[offset + 2],
            avcc[offset + 3],
        ]) as usize;
        offset += 4;
        if offset + length > avcc.len() {
            break;
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&avcc[offset..offset + length]);
        offset += length;
    }
}

/// Build an ISO/IEC 14496-15 `avcC` record from a SPS/PPS pair.
fn build_avcc_record(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(sps.len() + pps.len() + 11);
    out.push(0x01); // configurationVersion
    out.extend_from_slice(&sps[1..4]); // profile, compatibility, level
    out.push(0xFF); // reserved | lengthSizeMinusOne = 3 (4-byte lengths)
    out.push(0xE1); // reserved | numOfSequenceParameterSets = 1
    out.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    out.extend_from_slice(sps);
    out.push(0x01); // numOfPictureParameterSets
    out.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    out.extend_from_slice(pps);
    out
}

/// A sample is a sync frame unless it is explicitly flagged `NotSync`.
///
/// The attachment dictionaries are untyped, so this goes through the raw CF
/// entry point rather than the generically typed `CFDictionary` wrapper.
fn sample_is_keyframe(sample: &CMSampleBuffer) -> bool {
    unsafe extern "C-unwind" {
        fn CFDictionaryContainsKey(dict: *const c_void, key: *const c_void) -> u8;
    }

    let Some(attachments) = (unsafe { sample.sample_attachments_array(false) }) else {
        return true;
    };
    if attachments.count() == 0 {
        return true;
    }
    let entry = unsafe { attachments.value_at_index(0) };
    if entry.is_null() {
        return true;
    }
    let key = unsafe { objc2_core_media::kCMSampleAttachmentKey_NotSync };
    unsafe { CFDictionaryContainsKey(entry, key as *const CFString as *const c_void) == 0 }
}

/// Copy the compressed bytes out of the sample's block buffer.
fn sample_bytes(sample: &CMSampleBuffer) -> Option<Vec<u8>> {
    let block = unsafe { sample.data_buffer() }?;
    let total = unsafe { block.data_length() };
    if total == 0 {
        return None;
    }
    let mut out = vec![0u8; total];
    let destination = NonNull::new(out.as_mut_ptr() as *mut c_void)?;
    let status = unsafe { block.copy_data_bytes(0, total, destination) };
    (status == 0).then_some(out)
}

/// Pull SPS (index 0) and PPS (index 1) out of a format description.
fn extract_parameter_sets(
    description: &objc2_core_media::CMFormatDescription,
) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut sets = Vec::with_capacity(2);
    for index in 0..2usize {
        let mut pointer: *const u8 = std::ptr::null();
        let mut size: usize = 0;
        let status = unsafe {
            CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
                description,
                index,
                &mut pointer,
                &mut size,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if status != 0 || pointer.is_null() || size == 0 {
            return None;
        }
        sets.push(unsafe { std::slice::from_raw_parts(pointer, size) }.to_vec());
    }
    let pps = sets.pop()?;
    let sps = sets.pop()?;
    // The avcC record copies profile/compatibility/level out of the SPS.
    (sps.len() >= 4).then_some((sps, pps))
}

fn set_property(session: &VTCompressionSession, key: &str, value: &CFType) {
    let key = CFString::from_str(key);
    unsafe { VTSessionSetProperty(session, &key, Some(value)) };
}

fn set_bool(session: &VTCompressionSession, key: &str, value: bool) {
    set_property(session, key, CFBoolean::new(value).as_ref());
}

fn set_i32(session: &VTCompressionSession, key: &str, value: i32) {
    set_property(session, key, CFNumber::new_i32(value).as_ref());
}

fn set_string(session: &VTCompressionSession, key: &str, value: &str) {
    set_property(session, key, CFString::from_str(value).as_ref());
}

fn set_f64(session: &VTCompressionSession, key: &str, value: f64) {
    set_property(session, key, CFNumber::new_f64(value).as_ref());
}
