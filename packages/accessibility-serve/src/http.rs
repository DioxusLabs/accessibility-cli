//! HTTP and WebSocket surface.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use accessibility_core::video::FrameKind;

use crate::avcc;
use crate::input::{InputCommand, Orientation};
use crate::session::SimSession;
use crate::settings::{Setting, SettingKey};
use crate::webrtc_stream::WebRtcEngine;

const INDEX_HTML: &str = include_str!("../static/index.html");

#[derive(Clone)]
pub struct AppState {
    pub session: Arc<SimSession>,
    pub webrtc: Arc<WebRtcEngine>,
    pub default_transport: String,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/config", get(config))
        .route("/api/ax/tree", get(ax_tree))
        .route("/api/ax/hit", get(ax_hit))
        .route("/api/stats", get(stats))
        .route("/api/settings", get(settings).post(set_setting))
        .route("/api/orientation", post(set_orientation))
        .route("/webrtc/offer", post(webrtc_offer))
        .route("/ws/stream", get(stream_socket))
        .route("/ws/input", get(input_socket))
        .with_state(state)
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

#[derive(Serialize)]
struct ConfigResponse {
    udid: String,
    /// Raw framebuffer size. Constant regardless of orientation.
    width: u32,
    height: u32,
    orientation: Orientation,
    default_transport: String,
    transports: Vec<&'static str>,
    home_indicator_band: f64,
}

async fn config(State(state): State<AppState>) -> Json<ConfigResponse> {
    let device = state.session.device_info();
    Json(ConfigResponse {
        udid: device.udid,
        width: device.width,
        height: device.height,
        orientation: device.orientation,
        default_transport: state.default_transport.clone(),
        transports: vec!["webrtc", "h264"],
        home_indicator_band: crate::input::HOME_INDICATOR_BAND,
    })
}

async fn stats(State(state): State<AppState>) -> Json<crate::session::StatsReport> {
    Json(state.session.stats())
}

async fn settings(State(state): State<AppState>) -> Json<Vec<Setting>> {
    // Each read shells out to simctl, so keep it off the async worker threads.
    let session = Arc::clone(&state.session);
    Json(
        tokio::task::spawn_blocking(move || session.settings())
            .await
            .unwrap_or_default(),
    )
}

#[derive(Deserialize)]
struct SettingRequest {
    key: SettingKey,
    value: String,
}

async fn set_setting(
    State(state): State<AppState>,
    Json(request): Json<SettingRequest>,
) -> Response {
    let session = Arc::clone(&state.session);
    let result =
        tokio::task::spawn_blocking(move || session.set_setting(request.key, &request.value)).await;

    match result {
        Ok(Ok(value)) => Json(serde_json::json!({ "value": value })).into_response(),
        Ok(Err(error)) => (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
        Err(error) => internal_error(anyhow::anyhow!(error)),
    }
}

#[derive(Deserialize)]
struct OrientationRequest {
    orientation: Orientation,
}

async fn set_orientation(
    State(state): State<AppState>,
    Json(request): Json<OrientationRequest>,
) -> Response {
    state.session.set_orientation(request.orientation);
    Json(serde_json::json!({ "orientation": request.orientation })).into_response()
}

/// Map an `anyhow` error onto a 500 with the message preserved.
///
/// These are developer-facing diagnostics on a locally served tool, so the
/// detail is more useful than it would be on a public endpoint.
fn internal_error(error: anyhow::Error) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response()
}

#[derive(Deserialize)]
struct TreeQuery {
    /// Sweep the regions the tree walk cannot explain, reaching web content.
    #[serde(default)]
    scan: bool,
}

async fn ax_tree(State(state): State<AppState>, Query(query): Query<TreeQuery>) -> Response {
    match state.session.ax_snapshot(query.scan).await {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(error) => internal_error(error),
    }
}

#[derive(Deserialize)]
struct HitQuery {
    x: f64,
    y: f64,
}

async fn ax_hit(State(state): State<AppState>, Query(query): Query<HitQuery>) -> Response {
    match state.session.ax_hit_test(query.x, query.y).await {
        Ok(element) => Json(element).into_response(),
        Err(error) => internal_error(error),
    }
}

#[derive(Deserialize)]
struct OfferRequest {
    sdp: String,
}

#[derive(Serialize)]
struct AnswerResponse {
    sdp: String,
}

async fn webrtc_offer(State(state): State<AppState>, Json(offer): Json<OfferRequest>) -> Response {
    match state
        .webrtc
        .answer(Arc::clone(&state.session), offer.sdp)
        .await
    {
        Ok(sdp) => Json(AnswerResponse { sdp }).into_response(),
        Err(error) => internal_error(error),
    }
}

async fn stream_socket(
    State(state): State<AppState>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    upgrade.on_upgrade(move |socket| pump_h264(state, socket))
}

/// Raw H.264 transport for browsers driving WebCodecs directly.
///
/// This is the "expose a port and point something at it" path: no signaling,
/// no ICE, just length-prefixed frames. Also the fallback when WebRTC codec
/// negotiation fails.
async fn pump_h264(state: AppState, mut socket: WebSocket) {
    let mut frames = state.session.subscribe();
    let mut sent_parameter_set = false;

    loop {
        let frame = match frames.recv().await {
            Ok(frame) => frame,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                state.session.note_lag();
                state.session.request_keyframe();
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };

        // The encoder runs in Annex-B for WebRTC, so parameter sets arrive
        // inline on each IDR rather than as their own frame.
        if frame.kind == FrameKind::ParameterSet {
            continue;
        }

        // A decoder cannot start on a delta frame, and it needs the avcC
        // record before anything else.
        if !sent_parameter_set {
            let Some((sps, pps)) = avcc::parameter_sets(&frame.data) else {
                continue;
            };
            let record = avcc::avcc_record(&sps, &pps);
            let message = avcc::envelope(avcc::tag::PARAMETER_SET, &record);
            if socket.send(Message::Binary(message.into())).await.is_err() {
                break;
            }
            sent_parameter_set = true;
        }

        let tag = match frame.kind {
            FrameKind::Keyframe => avcc::tag::KEYFRAME,
            _ => avcc::tag::DELTA,
        };
        let payload = avcc::to_avcc(&frame.data);
        if payload.is_empty() {
            continue;
        }
        let message = avcc::envelope(tag, &payload);
        if socket.send(Message::Binary(message.into())).await.is_err() {
            break;
        }
    }
}

async fn input_socket(
    State(state): State<AppState>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    upgrade.on_upgrade(move |socket| pump_input(state, socket))
}

async fn pump_input(state: AppState, mut socket: WebSocket) {
    use futures_util::StreamExt;

    while let Some(Ok(message)) = socket.next().await {
        let payload = match message {
            Message::Text(text) => text.to_string(),
            Message::Binary(bytes) => match String::from_utf8(bytes.to_vec()) {
                Ok(text) => text,
                Err(_) => continue,
            },
            Message::Close(_) => break,
            _ => continue,
        };

        match serde_json::from_str::<InputCommand>(&payload) {
            Ok(command) => state.session.send_input(command),
            Err(error) => tracing::debug!("ignoring malformed input event: {error}"),
        }
    }
}
