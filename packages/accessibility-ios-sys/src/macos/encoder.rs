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
use objc2_core_video::CVImageBuffer;
use objc2_video_toolbox::{VTCompressionSession, VTEncodeInfoFlags, VTSessionSetProperty};

use super::pixel_buffer::cf_dict;

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
}

#[derive(Debug, Clone, Copy)]
pub struct EncoderConfig {
    pub fps: u32,
    pub bitrate: u32,
    /// Seconds between scheduled keyframes.
    pub keyframe_interval_secs: u32,
    pub nal_format: NalFormat,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            fps: 60,
            bitrate: 6_000_000,
            keyframe_interval_secs: 2,
            nal_format: NalFormat::AnnexB,
        }
    }
}

/// Sink for encoded chunks, invoked on the capture queue.
pub type ChunkSink = Arc<dyn Fn(EncodedChunk) + Send + Sync>;

pub struct H264Encoder {
    session: Option<CFRetained<VTCompressionSession>>,
    config: EncoderConfig,
    dimensions: (i32, i32),
    /// Frame index, used to synthesize presentation timestamps.
    frame_index: i64,
    /// Set by RTCP PLI/FIR so the next frame is forced to an IDR.
    force_keyframe: Arc<AtomicBool>,
    /// Whether the `avcC` record for the current session has been emitted.
    emitted_parameter_set: Arc<Mutex<bool>>,
    sink: ChunkSink,
}

// VideoToolbox sessions are documented as thread-safe, and this one is only
// driven from the capture queue regardless.
unsafe impl Send for H264Encoder {}

impl H264Encoder {
    pub fn new(config: EncoderConfig, force_keyframe: Arc<AtomicBool>, sink: ChunkSink) -> Self {
        Self {
            session: None,
            config,
            dimensions: (0, 0),
            frame_index: 0,
            force_keyframe,
            emitted_parameter_set: Arc::new(Mutex::new(false)),
            sink,
        }
    }

    /// Encode one frame, rebuilding the session if the source resized.
    pub fn encode(&mut self, image: &CVImageBuffer, width: i32, height: i32) -> Result<()> {
        if self.session.is_none() || self.dimensions != (width, height) {
            self.dimensions = (width, height);
            self.rebuild_session()?;
        }
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
                for chunk in package_sample(sample, nal_format, &emitted) {
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
        // emit no B-frames. It is not available on every encoder, so a plain
        // session is an acceptable fallback.
        let low_latency = cf_dict(&[(
            unsafe { objc2_video_toolbox::kVTVideoEncoderSpecification_EnableLowLatencyRateControl },
            CFBoolean::new(true).as_ref(),
        )]);

        let mut status = unsafe {
            VTCompressionSession::create(
                kCFAllocatorDefault,
                width,
                height,
                CODEC_H264,
                Some(&low_latency),
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
        set_bool(&session, "AllowFrameReordering", false);
        set_i32(&session, "MaxFrameDelayCount", 0);
        // Baseline is the safest bet for WebRTC: every browser negotiates it,
        // and we gain nothing from High on a UI stream with no B-frames.
        set_string(&session, "ProfileLevel", "H264_Baseline_AutoLevel");
        set_i32(&session, "ExpectedFrameRate", self.config.fps as i32);
        set_i32(&session, "MaxKeyFrameInterval", keyframe_interval as i32);
        set_i32(&session, "AverageBitRate", self.config.bitrate as i32);

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
