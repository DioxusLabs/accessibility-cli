use std::io::Read;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use accessibility_android_sys::AdbClient;
use accessibility_android_sys::emulator::screenrecord::{
    AnnexBAccessUnitParser, H264AccessUnit, ScreenRecordConfig, spawn_screenrecord,
};
use anyhow::{Context, Result, bail};

const IDLE_FLUSH: Duration = Duration::from_millis(75);
const PROBE_DURATION: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> Result<()> {
    let serial = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "emulator-5554".to_string());
    let adb = AdbClient::discover(Some(&serial));
    adb.check_connection().await?;
    let (width, height) = adb.get_screen_size().await?;
    let config = ScreenRecordConfig::for_max_dimension(width, height, Some(1280), 5_000_000);
    println!("device    : {serial}");
    println!("source    : {width}x{height}");
    println!("encoded   : {}x{}", config.width, config.height);

    let stimulus = adb.clone();
    let stimulus = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let _ = stimulus
            .swipe(
                (width as f64 * 0.5, height as f64 * 0.75),
                (width as f64 * 0.5, height as f64 * 0.25),
                800,
            )
            .await;
    });

    let started = Instant::now();
    let frames = capture(&adb, config, PROBE_DURATION, false)?;
    let elapsed = started.elapsed();
    let keyframes = frames.iter().filter(|frame| frame.keyframe).count();
    let bytes = frames.iter().map(|frame| frame.data.len()).sum::<usize>();
    if frames.is_empty() {
        bail!("screenrecord produced no complete H.264 frames");
    }
    if keyframes == 0 {
        bail!("screenrecord produced no H.264 keyframe");
    }
    println!("frames    : {}", frames.len());
    println!("keyframes : {keyframes}");
    println!("bytes     : {bytes}");
    println!(
        "fps       : {:.1}",
        frames.len() as f64 / elapsed.as_secs_f64()
    );
    println!(
        "bitrate   : {:.2} Mbps",
        bytes as f64 * 8.0 / elapsed.as_secs_f64() / 1_000_000.0
    );

    let restart = Instant::now();
    let restarted = capture(&adb, config, Duration::from_secs(2), true)?;
    let first = restarted
        .iter()
        .find(|frame| frame.keyframe)
        .context("fresh screenrecord session produced no keyframe")?;
    println!(
        "restart   : {:.1} ms",
        restart.elapsed().as_secs_f64() * 1000.0
    );
    println!("entry NALs: {:?}", nal_types(&first.data));
    let _ = stimulus.await;
    println!("probe passed");
    Ok(())
}

fn capture(
    adb: &AdbClient,
    config: ScreenRecordConfig,
    duration: Duration,
    stop_on_keyframe: bool,
) -> Result<Vec<H264AccessUnit>> {
    let mut child = spawn_screenrecord(adb, config)?;
    let mut stdout = child
        .stdout
        .take()
        .context("screenrecord stdout was not piped")?;
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(8);
    let reader = std::thread::spawn(move || {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            match stdout.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if tx.send(buffer[..read].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let started = Instant::now();
    let mut parser = AnnexBAccessUnitParser::default();
    let mut frames = Vec::new();
    while started.elapsed() < duration {
        match rx.recv_timeout(IDLE_FLUSH) {
            Ok(chunk) => frames.extend(parser.push(&chunk)),
            Err(mpsc::RecvTimeoutError::Timeout) => frames.extend(parser.flush_idle()),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if stop_on_keyframe && frames.iter().any(|frame| frame.keyframe) {
            break;
        }
    }
    frames.extend(parser.flush_idle());
    child.kill().context("failed to stop screenrecord")?;
    let _ = child.wait();
    reader
        .join()
        .map_err(|_| anyhow::anyhow!("screenrecord reader panicked"))?;
    Ok(frames)
}

fn nal_types(data: &[u8]) -> Vec<u8> {
    let mut types = Vec::new();
    let mut index = 0;
    while index + 3 < data.len() {
        let code_len = if data[index..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if data[index..].starts_with(&[0, 0, 1]) {
            3
        } else {
            index += 1;
            continue;
        };
        if let Some(header) = data.get(index + code_len) {
            types.push(header & 0x1f);
        }
        index += code_len + 1;
    }
    types
}
