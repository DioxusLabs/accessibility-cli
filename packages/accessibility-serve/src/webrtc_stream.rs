//! WebRTC video transport.
//!
//! Signaling is a single HTTP round trip: the browser posts a complete offer,
//! we answer with a complete SDP once ICE gathering finishes. No trickle, no
//! long-lived signaling socket. For a locally served simulator that is enough,
//! and it keeps the browser side to about thirty lines.
//!
//! Each viewer gets its own peer connection and its own forwarding task, so
//! one stalled client cannot wedge the others.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MIME_TYPE_H264, MediaEngine};
use webrtc::api::{API, APIBuilder};
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::media::Sample;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtcp::payload_feedbacks::full_intra_request::FullIntraRequest;
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;

use accessibility_core::video::FrameKind;

use crate::session::SimSession;

pub struct WebRtcEngine {
    api: API,
    config: RTCConfiguration,
}

impl WebRtcEngine {
    pub fn new(ice_servers: Vec<String>) -> Result<Self> {
        let mut media_engine = MediaEngine::default();
        media_engine.register_default_codecs()?;

        let registry = register_default_interceptors(Registry::new(), &mut media_engine)?;
        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .build();

        let config = RTCConfiguration {
            ice_servers: if ice_servers.is_empty() {
                // A simulator served on loopback or a LAN needs no STUN; host
                // candidates are sufficient and avoid a pointless round trip
                // to a public server.
                Vec::new()
            } else {
                vec![RTCIceServer {
                    urls: ice_servers,
                    ..Default::default()
                }]
            },
            ..Default::default()
        };

        Ok(Self { api, config })
    }

    /// Answer a browser offer, wiring a fresh track to the capture stream.
    pub async fn answer(&self, session: Arc<SimSession>, offer_sdp: String) -> Result<String> {
        let peer = Arc::new(self.api.new_peer_connection(self.config.clone()).await?);

        let track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_H264.to_owned(),
                ..Default::default()
            },
            "video".to_owned(),
            format!("sim-{}", session.device_info().udid),
        ));

        let sender = peer
            .add_track(Arc::clone(&track) as Arc<dyn TrackLocal + Send + Sync>)
            .await?;

        // Sender RTCP has to be drained or feedback never gets processed. PLI
        // and FIR both mean "I cannot decode, send me a fresh IDR".
        {
            let session = Arc::clone(&session);
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 1500];
                while let Ok((packets, _)) = sender.read(&mut buffer).await {
                    for packet in packets {
                        let any = packet.as_any();
                        if any.downcast_ref::<PictureLossIndication>().is_some()
                            || any.downcast_ref::<FullIntraRequest>().is_some()
                        {
                            session.request_keyframe();
                        }
                    }
                }
            });
        }

        let forwarder = spawn_forwarder(Arc::clone(&session), Arc::clone(&track));

        // Tear the forwarding task down when the viewer goes away, otherwise
        // every reconnect would leak a subscriber on the broadcast channel.
        {
            let forwarder = Arc::new(std::sync::Mutex::new(Some(forwarder)));
            peer.on_peer_connection_state_change(Box::new(move |state| {
                if matches!(
                    state,
                    RTCPeerConnectionState::Failed
                        | RTCPeerConnectionState::Disconnected
                        | RTCPeerConnectionState::Closed
                ) && let Some(handle) = forwarder.lock().unwrap().take()
                {
                    handle.abort();
                }
                Box::pin(async {})
            }));
        }

        peer.set_remote_description(RTCSessionDescription::offer(offer_sdp)?)
            .await?;
        let answer = peer.create_answer(None).await?;

        let mut gathering_complete = peer.gathering_complete_promise().await;
        peer.set_local_description(answer).await?;
        let _ = gathering_complete.recv().await;

        peer.local_description()
            .await
            .map(|description| description.sdp)
            .ok_or_else(|| anyhow!("WebRTC produced no local description"))
    }
}

/// Pump encoded frames from the capture broadcast onto a viewer's track.
fn spawn_forwarder(
    session: Arc<SimSession>,
    track: Arc<TrackLocalStaticSample>,
) -> tokio::task::JoinHandle<()> {
    let mut frames = session.subscribe();

    tokio::spawn(async move {
        // Timestamps are derived from arrival rather than a fixed cadence,
        // because the simulator only paints when something changes: an idle
        // screen produces ~5fps and a busy one ~60fps.
        let mut previous = tokio::time::Instant::now();

        loop {
            let frame = match frames.recv().await {
                Ok(frame) => frame,
                // Lagging just means this viewer fell behind; the next
                // keyframe will resynchronize it.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    session.note_lag();
                    session.request_keyframe();
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };

            // In Annex-B the parameter sets ride inline ahead of each IDR, so
            // there is nothing separate to send.
            if frame.kind == FrameKind::ParameterSet {
                continue;
            }

            let now = tokio::time::Instant::now();
            let duration = now.duration_since(previous);
            previous = now;

            let sample = Sample {
                data: frame.data,
                duration,
                ..Default::default()
            };
            if track.write_sample(&sample).await.is_err() {
                break;
            }
        }
    })
}
