use std::sync::Arc;
use std::time::{Duration, Instant};

use accessibility_android_sys::emulator::protocol::rtc::Id;
use accessibility_android_sys::emulator::{EmulatorGrpcClient, discover_emulator};
use anyhow::{Context, Result, anyhow, bail};
use rtp::codecs::h264::H264Packet;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MIME_TYPE_H264, MediaEngine};
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::interceptor::registry::Registry;
use webrtc::media::io::sample_builder::SampleBuilder;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::sdp_type::RTCSdpType;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::rtp_transceiver::RTCPFeedback;
use webrtc::rtp_transceiver::rtp_codec::{
    RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType,
};
use webrtc::track::track_remote::TrackRemote;

const TRACK_TIMEOUT: Duration = Duration::from_secs(15);
const KEYFRAME_TIMEOUT: Duration = Duration::from_secs(10);
const RECOVERY_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> Result<()> {
    let selector = std::env::args().nth(1);
    let discovery = discover_emulator(selector.as_deref()).await?;
    println!("discovery : {}", discovery.path.display());
    println!("endpoint  : {}", discovery.endpoint());

    let mut grpc = EmulatorGrpcClient::connect(discovery).await?;
    let status = grpc.status().await?;
    println!("emulator  : {}", status.version);
    println!("booted    : {}", status.booted);
    if !status.booted {
        bail!("Android Emulator has not finished booting");
    }
    if let (Some(width), Some(height)) = (
        status.platform_config.get("hw.lcd.width"),
        status.platform_config.get("hw.lcd.height"),
    ) {
        println!("display   : {width}x{height}");
    }

    let id = grpc.begin_rtc_stream().await?;
    println!("rtc id    : {}", id.guid);
    let messages = grpc.receive_jsep_stream(id.clone()).await?;
    let peer = create_peer().await?;
    let (track_tx, mut track_rx) = mpsc::channel(1);
    peer.on_track(Box::new(move |track, _, _| {
        let track_tx = track_tx.clone();
        Box::pin(async move {
            let _ = track_tx.send(track).await;
        })
    }));

    let signaling_peer = Arc::clone(&peer);
    let signaling_grpc = grpc.clone();
    let signaling_id = id.clone();
    let mut signaling = tokio::spawn(async move {
        run_signaling(signaling_peer, signaling_grpc, signaling_id, messages).await
    });

    let track = tokio::select! {
        track = track_rx.recv() => track.ok_or_else(|| anyhow!("WebRTC peer closed before receiving a track"))?,
        result = &mut signaling => return result.context("signaling task panicked")?,
        _ = tokio::time::sleep(TRACK_TIMEOUT) => bail!("timed out waiting for the emulator video track"),
    };
    let codec = track.codec().capability;
    println!("codec     : {} {}", codec.mime_type, codec.sdp_fmtp_line);
    if !codec.mime_type.eq_ignore_ascii_case(MIME_TYPE_H264) {
        bail!("emulator negotiated {}, not H.264", codec.mime_type);
    }

    let report = read_frames(&peer, &track).await?;
    println!("frames    : {}", report.frames);
    println!("keyframes : {}", report.keyframes);
    println!("bytes     : {}", report.bytes);
    println!(
        "fps       : {:.1}",
        report.frames as f64 / report.elapsed.as_secs_f64()
    );
    println!(
        "bitrate   : {:.2} Mbps",
        report.bytes as f64 * 8.0 / report.elapsed.as_secs_f64() / 1_000_000.0
    );
    println!(
        "pli->idr  : {:.1} ms",
        report.recovery.as_secs_f64() * 1000.0
    );

    grpc.end_rtc_stream(id).await?;
    peer.close().await?;
    signaling.abort();
    println!("probe passed");
    Ok(())
}

async fn create_peer() -> Result<Arc<RTCPeerConnection>> {
    let mut media_engine = MediaEngine::default();
    let feedback = vec![
        RTCPFeedback {
            typ: "nack".to_owned(),
            parameter: "".to_owned(),
        },
        RTCPFeedback {
            typ: "nack".to_owned(),
            parameter: "pli".to_owned(),
        },
        RTCPFeedback {
            typ: "ccm".to_owned(),
            parameter: "fir".to_owned(),
        },
    ];
    for (payload_type, profile) in [(102, "42001f"), (125, "42e01f"), (123, "640032")] {
        media_engine.register_codec(
            RTCRtpCodecParameters {
                capability: RTCRtpCodecCapability {
                    mime_type: MIME_TYPE_H264.to_owned(),
                    clock_rate: 90000,
                    channels: 0,
                    sdp_fmtp_line: format!(
                        "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id={profile}"
                    ),
                    rtcp_feedback: feedback.clone(),
                },
                payload_type,
                ..Default::default()
            },
            RTPCodecType::Video,
        )?;
    }
    let registry = register_default_interceptors(Registry::new(), &mut media_engine)?;
    let api = APIBuilder::new()
        .with_media_engine(media_engine)
        .with_interceptor_registry(registry)
        .build();
    Ok(Arc::new(
        api.new_peer_connection(RTCConfiguration::default()).await?,
    ))
}

async fn run_signaling(
    peer: Arc<RTCPeerConnection>,
    mut grpc: EmulatorGrpcClient,
    id: Id,
    mut messages: tonic::Streaming<
        accessibility_android_sys::emulator::protocol::rtc::ReceiveJsepMessageResponse,
    >,
) -> Result<()> {
    let mut remote_description_set = false;
    let mut pending_candidates = Vec::new();
    while let Some(response) = messages.message().await? {
        let Some(jsep) = response.jsep_msg else {
            continue;
        };
        if jsep.message.is_empty() {
            continue;
        }
        let message: Value = serde_json::from_str(&jsep.message)
            .with_context(|| format!("invalid emulator JSEP message: {}", jsep.message))?;
        if message.get("bye").is_some() {
            bail!("emulator ended the RTC stream before the probe completed");
        }
        if let Some(value) = message.get("candidate") {
            let candidate: RTCIceCandidateInit = serde_json::from_value(value.clone())?;
            if remote_description_set {
                peer.add_ice_candidate(candidate).await?;
            } else {
                pending_candidates.push(candidate);
            }
        }
        let Some(value) = message.get("sdp") else {
            continue;
        };
        let description: RTCSessionDescription = serde_json::from_value(value.clone())?;
        if description.sdp_type != RTCSdpType::Offer {
            continue;
        }
        peer.set_remote_description(description).await?;
        remote_description_set = true;
        for candidate in pending_candidates.drain(..) {
            peer.add_ice_candidate(candidate).await?;
        }
        let answer = peer.create_answer(None).await?;
        let mut gathering_complete = peer.gathering_complete_promise().await;
        peer.set_local_description(answer).await?;
        let _ = gathering_complete.recv().await;
        let answer = peer
            .local_description()
            .await
            .ok_or_else(|| anyhow!("WebRTC peer produced no local answer"))?;
        grpc.send_jsep(id.clone(), json!({ "sdp": answer }).to_string())
            .await?;
    }
    bail!("emulator JSEP stream closed")
}

struct ProbeReport {
    frames: u64,
    keyframes: u64,
    bytes: u64,
    elapsed: Duration,
    recovery: Duration,
}

async fn read_frames(peer: &RTCPeerConnection, track: &TrackRemote) -> Result<ProbeReport> {
    let started = Instant::now();
    let mut builder = SampleBuilder::new(16, H264Packet::default(), 90000)
        .with_max_time_delay(Duration::from_millis(250));
    let mut frames = 0;
    let mut keyframes = 0;
    let mut bytes = 0;
    let mut requested_at: Option<Instant> = None;

    loop {
        let timeout = if requested_at.is_some() {
            RECOVERY_TIMEOUT
        } else {
            KEYFRAME_TIMEOUT
        };
        let (packet, _) = tokio::time::timeout(timeout, track.read_rtp())
            .await
            .context("timed out waiting for H.264 RTP")??;
        builder.push(packet);
        while let Some(sample) = builder.pop() {
            let types = annex_b_nal_types(&sample.data);
            let keyframe = types.contains(&5);
            if keyframe && (!types.contains(&7) || !types.contains(&8)) {
                bail!("H.264 keyframe did not carry SPS and PPS");
            }
            frames += 1;
            bytes += sample.data.len() as u64;
            if !keyframe {
                continue;
            }
            keyframes += 1;
            if let Some(requested_at) = requested_at {
                return Ok(ProbeReport {
                    frames,
                    keyframes,
                    bytes,
                    elapsed: started.elapsed(),
                    recovery: requested_at.elapsed(),
                });
            }
            peer.write_rtcp(&[Box::new(PictureLossIndication {
                sender_ssrc: 0,
                media_ssrc: track.ssrc(),
            })])
            .await?;
            requested_at = Some(Instant::now());
        }
    }
}

fn annex_b_nal_types(data: &[u8]) -> Vec<u8> {
    let mut types = Vec::new();
    let mut index = 0;
    while index + 3 < data.len() {
        let start_len = if data[index..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if data[index..].starts_with(&[0, 0, 1]) {
            3
        } else {
            index += 1;
            continue;
        };
        if let Some(header) = data.get(index + start_len) {
            types.push(header & 0x1f);
        }
        index += start_len + 1;
    }
    types
}
