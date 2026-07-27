//! Probe the SimulatorKit framebuffer capture and H.264 encode path.
//!
//! Run with: `cargo run -p accessibility-ios-sys --example framebuffer_probe`

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("framebuffer_probe only runs on macOS");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use accessibility_ios_sys::{
        ChunkKind, EncodedChunk, EncoderConfig, NalFormat, SimVideoStream,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    let keyframes = Arc::new(AtomicU64::new(0));
    let deltas = Arc::new(AtomicU64::new(0));
    let parameter_sets = Arc::new(AtomicU64::new(0));
    let bytes = Arc::new(AtomicU64::new(0));
    let first_nal_ok = Arc::new(AtomicU64::new(0));

    let sink = {
        let (keyframes, deltas, parameter_sets, bytes, first_nal_ok) = (
            Arc::clone(&keyframes),
            Arc::clone(&deltas),
            Arc::clone(&parameter_sets),
            Arc::clone(&bytes),
            Arc::clone(&first_nal_ok),
        );
        Arc::new(move |chunk: EncodedChunk| {
            bytes.fetch_add(chunk.data.len() as u64, Ordering::Relaxed);
            match chunk.kind {
                ChunkKind::ParameterSet => parameter_sets.fetch_add(1, Ordering::Relaxed),
                ChunkKind::Keyframe => keyframes.fetch_add(1, Ordering::Relaxed),
                ChunkKind::Delta => deltas.fetch_add(1, Ordering::Relaxed),
            };
            // Every Annex-B access unit must open with a start code.
            if chunk.data.starts_with(&[0, 0, 0, 1]) {
                first_nal_ok.fetch_add(1, Ordering::Relaxed);
            }
        })
    };

    let config = EncoderConfig {
        nal_format: NalFormat::AnnexB,
        ..Default::default()
    };
    println!(
        "encode size for 1206x2622: {:?} (max_dimension = {:?})",
        config.encode_size(1206, 2622),
        config.max_dimension
    );
    let stream = SimVideoStream::start(None, config, sink)?;
    println!("device udid: {}", stream.device_udid());

    let started = Instant::now();
    let deadline = started + Duration::from_secs(5);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1000));
        let stats = stream.stats();
        println!(
            "frames={} {}x{} keyframes={} deltas={} kb={}",
            stats.frame_count,
            stats.width,
            stats.height,
            keyframes.load(Ordering::Relaxed),
            deltas.load(Ordering::Relaxed),
            bytes.load(Ordering::Relaxed) / 1024,
        );
    }

    let total_chunks = keyframes.load(Ordering::Relaxed) + deltas.load(Ordering::Relaxed);
    let elapsed = started.elapsed().as_secs_f64();

    println!("---");
    println!("encoded chunks : {total_chunks}");
    println!("keyframes      : {}", keyframes.load(Ordering::Relaxed));
    println!("annex-b valid  : {}", first_nal_ok.load(Ordering::Relaxed));
    println!(
        "bitrate        : {:.2} Mbps",
        (bytes.load(Ordering::Relaxed) as f64 * 8.0) / elapsed / 1_000_000.0
    );

    if total_chunks == 0 {
        anyhow::bail!("no encoded frames produced");
    }
    if keyframes.load(Ordering::Relaxed) == 0 {
        anyhow::bail!("no keyframes produced - decoders would never start");
    }
    if first_nal_ok.load(Ordering::Relaxed) != total_chunks {
        anyhow::bail!("some access units were not Annex-B framed");
    }
    println!("OK");
    Ok(())
}
