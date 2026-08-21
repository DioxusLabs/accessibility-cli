use std::fs::OpenOptions;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use accessibility_android_sys::AdbClient;
use accessibility_android_sys::emulator::protocol::controller::image_format::ImgFormat;
use accessibility_android_sys::emulator::protocol::controller::image_transport::TransportChannel;
use accessibility_android_sys::emulator::protocol::controller::{ImageFormat, ImageTransport};
use accessibility_android_sys::emulator::{EmulatorGrpcClient, discover_emulator};
use anyhow::{Context, Result, bail};
use memmap2::MmapOptions;

const MAX_DIMENSION: u32 = 1280;
const PROBE_DURATION: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> Result<()> {
    let selector = std::env::args().nth(1);
    let discovery = discover_emulator(selector.as_deref()).await?;
    let serial = discovery
        .properties
        .get("port.serial")
        .map(|port| format!("emulator-{port}"))
        .context("discovery file has no port.serial")?;
    let mut client = EmulatorGrpcClient::connect(discovery.clone()).await?;
    let status = client.status().await?;
    println!("emulator : {}", status.version);
    println!("endpoint : {}", discovery.endpoint());
    println!("serial   : {serial}");

    let stop = Arc::new(AtomicBool::new(false));
    let stimulus_stop = Arc::clone(&stop);
    let stimulus_serial = serial.clone();
    let stimulus = std::thread::spawn(move || {
        let adb = AdbClient::discover(Some(&stimulus_serial));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let Ok((width, height)) = runtime.block_on(adb.get_screen_size()) else {
            return;
        };
        while !stimulus_stop.load(Ordering::Relaxed) {
            let _ = runtime.block_on(adb.swipe(
                (width as f64 * 0.5, height as f64 * 0.75),
                (width as f64 * 0.5, height as f64 * 0.25),
                250,
            ));
            std::thread::sleep(Duration::from_millis(150));
        }
    });

    let bytes = probe_bytes(client.clone()).await?;
    print_report("grpc", &bytes);
    let mmap = probe_mmap(client).await?;
    print_report("mmap", &mmap);
    stop.store(true, Ordering::Relaxed);
    let _ = stimulus.join();
    println!("probe passed");
    Ok(())
}

#[derive(Default)]
struct Report {
    frames: u64,
    bytes: u64,
    gaps: u64,
    tears: u64,
    latency_us: Vec<u64>,
    width: u32,
    height: u32,
    elapsed: Duration,
}

async fn probe_bytes(mut client: EmulatorGrpcClient) -> Result<Report> {
    let format = request_format(None);
    let mut stream = client.stream_screenshots(format).await?;
    collect(&mut stream, None).await
}

async fn probe_mmap(mut client: EmulatorGrpcClient) -> Result<Report> {
    let path = std::env::temp_dir().join(format!(
        "accessibility-emulator-raw-{}.rgba",
        std::process::id()
    ));
    let capacity = MAX_DIMENSION as u64 * MAX_DIMENSION as u64 * 4;
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&path)?;
    file.set_len(capacity)?;
    let mut mapping = unsafe { MmapOptions::new().len(capacity as usize).map_mut(&file)? };
    let handle = format!("file://{}", path.display());
    let format = request_format(Some(ImageTransport {
        channel: TransportChannel::Mmap as i32,
        handle,
    }));
    let mut stream = client.stream_screenshots(format).await?;
    let report = collect(&mut stream, Some(&mut mapping)).await;
    drop(mapping);
    drop(file);
    let _ = std::fs::remove_file(path);
    report
}

fn request_format(transport: Option<ImageTransport>) -> ImageFormat {
    ImageFormat {
        format: ImgFormat::Rgba8888 as i32,
        rotation: None,
        width: MAX_DIMENSION,
        height: MAX_DIMENSION,
        display: 0,
        transport,
    }
}

async fn collect(
    stream: &mut tonic::Streaming<accessibility_android_sys::emulator::protocol::controller::Image>,
    mut mapping: Option<&mut memmap2::MmapMut>,
) -> Result<Report> {
    let started = Instant::now();
    let mut report = Report::default();
    let mut previous_seq = None;
    while started.elapsed() < PROBE_DURATION {
        let image = match tokio::time::timeout(Duration::from_secs(2), stream.message()).await {
            Ok(Ok(Some(image))) => image,
            Ok(Ok(None)) => break,
            Ok(Err(error)) => return Err(error.into()),
            Err(_) if report.frames > 0 => continue,
            Err(error) => return Err(error).context("timed out waiting for emulator frame"),
        };
        let format = image.format.context("emulator frame has no format")?;
        let width = format.width.max(image.width);
        let height = format.height.max(image.height);
        if width == 0 || height == 0 {
            continue;
        }
        let expected = width as usize * height as usize * 4;
        if let Some(mapping) = mapping.as_deref_mut() {
            if expected > mapping.len() {
                bail!("mapped frame {width}x{height} exceeds mapping");
            }
            let first = checksum(&mapping[..expected]);
            let owned = mapping[..expected].to_vec();
            let second = checksum(&mapping[..expected]);
            report.tears += u64::from(first != second || first != checksum(&owned));
            report.bytes += owned.len() as u64;
        } else {
            if image.image.len() != expected {
                bail!(
                    "RGBA frame is {} bytes, expected {expected} for {width}x{height}",
                    image.image.len()
                );
            }
            report.bytes += image.image.len() as u64;
        }
        if let Some(previous) = previous_seq {
            report.gaps += image.seq.wrapping_sub(previous).saturating_sub(1) as u64;
        }
        previous_seq = Some(image.seq);
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_micros() as u64;
        if image.timestamp_us > 0 && now >= image.timestamp_us {
            report.latency_us.push(now - image.timestamp_us);
        }
        report.frames += 1;
        report.width = width;
        report.height = height;
    }
    report.elapsed = started.elapsed();
    Ok(report)
}

fn checksum(bytes: &[u8]) -> u64 {
    bytes
        .chunks(4096)
        .map(|chunk| chunk.first().copied().unwrap_or_default() as u64)
        .sum()
}

fn print_report(name: &str, report: &Report) {
    let mean_latency = if report.latency_us.is_empty() {
        0.0
    } else {
        report.latency_us.iter().sum::<u64>() as f64 / report.latency_us.len() as f64
    };
    println!("{name}:");
    println!("  geometry : {}x{}", report.width, report.height);
    println!("  frames   : {}", report.frames);
    println!(
        "  fps      : {:.1}",
        report.frames as f64 / report.elapsed.as_secs_f64()
    );
    println!("  copied   : {} bytes", report.bytes);
    println!("  seq gaps : {}", report.gaps);
    println!("  tears    : {}", report.tears);
    println!("  latency  : {:.1} ms", mean_latency / 1000.0);
}
