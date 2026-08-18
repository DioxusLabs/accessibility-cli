use std::time::{Duration, Instant};

use accessibility_core::platform::android::AdbClient;
use accessibility_core::platform::android::input::{
    HardwareButton, InputCommand, Orientation, TouchPhase,
};
use accessibility_core::platform::android::session::EmulatorSession;
use accessibility_core::video::{FrameKind, VideoConfig};
use anyhow::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let requested_serial = std::env::args().nth(1);
    let adb = AdbClient::discover(requested_serial.as_deref());
    let serial = adb.resolved_serial()?;
    let _ = adb.stop_app("com.google.android.settings.intelligence");
    let _ = adb.stop_app("com.android.settings");
    adb.launch_app("com.android.settings", Some(".Settings"))?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let session = EmulatorSession::start(Some(&serial), VideoConfig::default())?;
    session.seed_orientation().await;
    let device = session.device_info();
    println!("device     : {}", device.serial);
    println!("geometry   : {}x{}", device.width, device.height);
    println!("orientation: {:?}", device.orientation);

    let snapshot = session.ax_snapshot(false).await?;
    println!("app        : {:?}", snapshot.app_name);
    println!("elements   : {}", snapshot.elements.len());
    println!("coverage   : {:.1}%", snapshot.coverage * 100.0);
    if let Some(element) = session.ax_hit_test(0.5, 0.5).await? {
        println!("center     : {} {}", element.role, element.selector);
    }
    let tappable = snapshot
        .elements
        .iter()
        .find(|element| {
            element.actions.iter().any(|action| action == "click") && element.bounds.is_some()
        })
        .context("Settings tree has no tappable element")?;
    let bounds = tappable.bounds.unwrap();
    let tap_x = bounds.x + bounds.width / 2.0;
    let tap_y = bounds.y + bounds.height / 2.0;
    println!("tap        : {}", tappable.selector);
    session.send_input(InputCommand::Touch {
        phase: TouchPhase::Begin,
        x: tap_x,
        y: tap_y,
    });
    session.send_input(InputCommand::Touch {
        phase: TouchPhase::End,
        x: tap_x,
        y: tap_y,
    });
    tokio::time::sleep(Duration::from_millis(700)).await;
    let tapped = session.ax_snapshot(false).await?;
    if tapped
        .elements
        .iter()
        .map(|element| &element.selector)
        .eq(snapshot.elements.iter().map(|element| &element.selector))
    {
        anyhow::bail!("gRPC touch input did not change the Settings tree");
    }

    session.send_input(InputCommand::Button {
        button: HardwareButton::Home,
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    let home = session.ax_snapshot(false).await?;
    if home.app_name == tapped.app_name {
        anyhow::bail!("gRPC Home input did not leave the tapped Settings surface");
    }
    println!("after Home : {:?}", home.app_name);
    let mut frames = session.subscribe();
    let started = Instant::now();
    let mut received = 0;
    let mut keyframes = 0;
    let mut bytes = 0;
    while started.elapsed() < Duration::from_secs(5) {
        let frame = match tokio::time::timeout(Duration::from_secs(2), frames.recv()).await {
            Ok(frame) => frame?,
            Err(_) if received > 0 => break,
            Err(error) => {
                return Err(error).context("timed out waiting for an Android Emulator frame");
            }
        };
        received += 1;
        bytes += frame.data.len();
        if frame.kind == FrameKind::Keyframe {
            keyframes += 1;
        }
        if received >= 30 {
            break;
        }
    }
    let stats = session.stats();
    println!("received   : {received} frames, {keyframes} key, {bytes} bytes");
    println!("session    : {:.1} fps, {:.2} Mbps", stats.fps, stats.mbps);

    session.set_orientation(Orientation::LandscapeRight)?;
    let landscape = session.ax_snapshot(false).await?;
    let landscape_stats = session.stats();
    if !landscape.is_landscape || landscape_stats.encoded_width <= landscape_stats.encoded_height {
        anyhow::bail!("Android Emulator did not reconfigure for landscape");
    }
    println!(
        "landscape  : {}x{}",
        landscape_stats.encoded_width, landscape_stats.encoded_height
    );
    session.set_orientation(Orientation::Portrait)?;
    let portrait = session.ax_snapshot(false).await?;
    let portrait_stats = session.stats();
    if portrait.is_landscape || portrait_stats.encoded_width >= portrait_stats.encoded_height {
        anyhow::bail!("Android Emulator did not reconfigure for portrait");
    }
    println!(
        "portrait   : {}x{}",
        portrait_stats.encoded_width, portrait_stats.encoded_height
    );
    println!("probe passed");
    Ok(())
}
