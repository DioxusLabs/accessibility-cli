//! Serve a live, interactive iOS Simulator stream in the browser.
//!
//! Captures the simulator framebuffer, encodes it with VideoToolbox, and
//! offers it over WebRTC (default) or as raw H.264 for browsers driving
//! WebCodecs. Pointer and keyboard input flow back over a WebSocket, and the
//! accessibility tree is exposed so the UI can inspect elements.

pub mod avcc;
pub mod ax;
pub mod http;
pub mod input;
pub mod keymap;
pub mod session;
pub mod settings;
pub mod webrtc_stream;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};

use accessibility_core::video::VideoConfig;

pub use session::SimSession;

/// Which transport the web UI should try first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transport {
    /// Real-time media track. Lowest latency, adapts to loss.
    #[default]
    WebRtc,
    /// Length-prefixed H.264 over a WebSocket, decoded with WebCodecs.
    /// Simpler to tunnel and to consume from non-browser clients.
    H264,
}

impl Transport {
    fn as_str(self) -> &'static str {
        match self {
            Transport::WebRtc => "webrtc",
            Transport::H264 => "h264",
        }
    }
}

impl std::str::FromStr for Transport {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "webrtc" => Ok(Transport::WebRtc),
            "h264" | "avcc" => Ok(Transport::H264),
            other => anyhow::bail!("Unknown transport '{other}' (expected webrtc or h264)"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ServeConfig {
    /// Simulator to serve. `None` picks the first booted device.
    pub udid: Option<String>,
    pub address: SocketAddr,
    pub transport: Transport,
    pub video: VideoConfig,
    /// ICE servers for WebRTC. Empty means host candidates only, which is
    /// correct for loopback and LAN use.
    pub ice_servers: Vec<String>,
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            udid: None,
            address: SocketAddr::from(([127, 0, 0, 1], 3200)),
            transport: Transport::default(),
            video: VideoConfig::default(),
            ice_servers: Vec::new(),
        }
    }
}

/// Start capturing and serve until the process is interrupted.
pub async fn serve(config: ServeConfig) -> Result<()> {
    let session = SimSession::start(config.udid.as_deref(), config.video)
        .context("failed to start simulator capture")?;
    // The framebuffer cannot reveal orientation, so ask accessibility once
    // before serving; otherwise an already-rotated device renders sideways.
    session.seed_orientation().await;
    let device = session.device_info();

    let webrtc = Arc::new(
        webrtc_stream::WebRtcEngine::new(config.ice_servers.clone())
            .context("failed to initialize WebRTC")?,
    );

    let state = http::AppState {
        session,
        webrtc,
        default_transport: config.transport.as_str().to_string(),
    };

    let listener = tokio::net::TcpListener::bind(config.address)
        .await
        .with_context(|| format!("failed to bind {}", config.address))?;
    let bound = listener.local_addr()?;

    println!("serving simulator {}", device.udid);
    println!("  transport : {}", config.transport.as_str());
    println!("  preview   : http://{bound}");

    axum::serve(listener, http::router(state))
        .await
        .context("server error")
}
