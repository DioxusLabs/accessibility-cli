//! Serve a live, interactive iOS Simulator stream in the browser.
//!
//! Captures the simulator framebuffer, encodes it with VideoToolbox, and
//! offers it over WebRTC (default) or as raw H.264 for browsers driving
//! WebCodecs. Pointer and keyboard input flow back over a WebSocket, and the
//! accessibility tree is exposed so the UI can inspect elements.

pub mod avcc;
#[cfg(target_os = "macos")]
pub mod ax;
#[cfg(target_os = "macos")]
pub mod coverage;
pub mod http;
#[cfg(target_os = "macos")]
pub mod input;
#[cfg(target_os = "macos")]
pub mod keymap;
pub mod session;
#[cfg(target_os = "macos")]
pub mod settings;
pub mod webrtc_stream;

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};

use accessibility_core::video::VideoConfig;

#[cfg(target_os = "macos")]
pub use accessibility_core::platform::ios_simulator::SimSession;

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

#[derive(Debug, Clone)]
pub struct ServeEmulatorConfig {
    pub serial: Option<String>,
    pub address: SocketAddr,
    pub transport: Transport,
    pub video: VideoConfig,
    pub ice_servers: Vec<String>,
}

impl Default for ServeEmulatorConfig {
    fn default() -> Self {
        Self {
            serial: None,
            address: SocketAddr::from(([127, 0, 0, 1], 3200)),
            transport: Transport::default(),
            video: VideoConfig::default(),
            ice_servers: Vec::new(),
        }
    }
}

/// Start capturing and serve until the process is interrupted.
#[cfg(target_os = "macos")]
pub async fn serve(config: ServeConfig) -> Result<()> {
    let session = SimSession::start(config.udid.as_deref(), config.video)
        .context("failed to start simulator capture")?;
    // The framebuffer cannot reveal orientation, so ask accessibility once
    // before serving; otherwise an already-rotated device renders sideways.
    let session = session::Session::ios(session);
    session.seed_orientation().await;
    serve_session(
        session,
        config.address,
        config.transport,
        config.ice_servers,
    )
    .await
}

/// iOS Simulator serving is only available on macOS.
#[cfg(not(target_os = "macos"))]
pub async fn serve(_config: ServeConfig) -> Result<()> {
    anyhow::bail!("Serving an iOS Simulator requires macOS")
}

pub async fn serve_emulator(config: ServeEmulatorConfig) -> Result<()> {
    let session = session::EmulatorSession::start(config.serial.as_deref(), config.video)
        .context("failed to start Android Emulator capture")?;
    let session = session::Session::android(session);
    session.seed_orientation().await;
    serve_session(
        session,
        config.address,
        config.transport,
        config.ice_servers,
    )
    .await
}

async fn serve_session(
    session: session::Session,
    address: SocketAddr,
    transport: Transport,
    ice_servers: Vec<String>,
) -> Result<()> {
    let device = session.device_info();
    let webrtc = Arc::new(
        webrtc_stream::WebRtcEngine::new(ice_servers).context("failed to initialize WebRTC")?,
    );
    let state = http::AppState {
        session,
        webrtc,
        default_transport: transport.as_str().to_string(),
    };
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("failed to bind {address}"))?;
    let bound = listener.local_addr()?;
    println!("serving {} {}", device.platform, device.id);
    println!("  transport : {}", transport.as_str());
    println!("  preview   : http://{bound}");
    axum::serve(listener, http::router(state))
        .await
        .context("server error")
}
