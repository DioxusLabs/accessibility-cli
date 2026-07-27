//! Platform-agnostic live video capture.
//!
//! This is the streaming counterpart to [`crate::accessibility::Screenshot`]:
//! where a screenshot is a single decoded image, a [`VideoCapture`] is a
//! continuous source of *encoded* frames.
//!
//! Deliberately encoded-frame oriented. Every platform that can do this at all
//! has a hardware encoder sitting right next to its capture API, and handing
//! raw surfaces across the abstraction boundary would force a copy and make
//! the zero-copy paths unreachable.
//!
//! Only the iOS Simulator backend is implemented today; every other platform
//! reports [`VideoCapture`] as unsupported.

use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;

/// What the encoder should optimize for.
///
/// A single choice rather than two knobs, because the underlying settings are
/// mutually exclusive: constant-quality rate control is ignored while
/// low-latency rate control is enabled. Expressing it this way makes the
/// combination that silently does nothing impossible to ask for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tuning {
    /// Live interactive streaming. Lowest latency; spends a fixed bitrate.
    Interactive {
        /// Target bitrate, or `None` to derive one from the encode resolution.
        bitrate: Option<u32>,
    },
    /// Recording and offline capture. Targets a constant quality from 0 to 1,
    /// letting the encoder buffer and letting the bitrate vary.
    Recording { quality: f64 },
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning::Interactive { bitrate: None }
    }
}

/// Compressed video codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoCodec {
    #[default]
    H264,
}

/// How H.264 NAL units are framed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NalFormat {
    /// `00 00 00 01` start codes. What WebRTC's H.264 payloader expects.
    #[default]
    AnnexB,
    /// 4-byte big-endian length prefixes, paired with a separate parameter set
    /// record. What the browser `VideoDecoder` API expects.
    Avcc,
}

/// What a given [`EncodedFrame`] carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    /// Codec configuration (an `avcC` record for H.264). Only produced in
    /// [`NalFormat::Avcc`]; in Annex-B the parameter sets are inline.
    ParameterSet,
    /// An independently decodable frame.
    Keyframe,
    /// A frame that depends on earlier frames.
    Delta,
}

impl FrameKind {
    /// Whether a client joining at this frame can start decoding.
    pub fn is_decodable_entry_point(self) -> bool {
        matches!(self, FrameKind::ParameterSet | FrameKind::Keyframe)
    }
}

/// One encoded unit, ready to be put on a wire.
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    pub data: Bytes,
    pub kind: FrameKind,
}

/// Pixel dimensions of the captured display.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScreenGeometry {
    pub width: u32,
    pub height: u32,
}

impl ScreenGeometry {
    pub fn is_valid(self) -> bool {
        self.width > 0 && self.height > 0
    }
}

/// Encoder tuning.
#[derive(Debug, Clone, Copy)]
pub struct VideoConfig {
    pub codec: VideoCodec,
    pub nal_format: NalFormat,
    pub fps: u32,
    /// What to optimize for. Deriving the bitrate is usually right: a fixed
    /// value that suits a phone framebuffer is wildly wrong for a watch, and
    /// too low a value does not just soften the image, it makes the encoder
    /// drop frames.
    pub tuning: Tuning,
    /// Longest edge to encode at; the source is scaled down to fit.
    ///
    /// Device framebuffers are far larger than any browser preview of them,
    /// and every extra pixel costs bitrate that would otherwise go into
    /// quality.
    pub max_dimension: Option<u32>,
    /// Seconds between scheduled keyframes. Shorter means faster recovery for
    /// clients that join late or drop packets, at the cost of bitrate.
    pub keyframe_interval_secs: u32,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            codec: VideoCodec::default(),
            nal_format: NalFormat::default(),
            fps: 60,
            tuning: Tuning::default(),
            max_dimension: Some(1280),
            keyframe_interval_secs: 2,
        }
    }
}

/// Sink for encoded frames.
///
/// Invoked on the platform's capture thread, so implementations must not
/// block. The expected shape is a bounded channel that drops on overflow: for
/// interactive streaming, a stale frame is worth less than a fresh one.
pub type FrameSink = Arc<dyn Fn(EncodedFrame) + Send + Sync>;

/// A running video capture session.
///
/// `Sync` is required because a session is shared across every connected
/// viewer. Implementations only expose atomics and locked state through
/// `&self`; anything that mutates the pipeline takes `&mut self`.
pub trait VideoCapture: Send + Sync {
    /// Pixel geometry of the source. May be zero until the first frame lands.
    fn geometry(&self) -> ScreenGeometry;

    /// Geometry actually being encoded, after any downscale.
    fn encoded_geometry(&self) -> ScreenGeometry {
        self.geometry()
    }

    /// Request that the next encoded frame be a keyframe.
    ///
    /// Called when a new client subscribes, or in response to an RTCP PLI/FIR
    /// from a WebRTC receiver.
    fn request_keyframe(&self);

    /// Stop capturing and release platform resources.
    fn stop(&mut self);
}

/// Error returned by platforms without a video backend.
pub fn unsupported<T>(platform: &str) -> Result<T> {
    anyhow::bail!("Video capture is not supported on {platform}")
}
