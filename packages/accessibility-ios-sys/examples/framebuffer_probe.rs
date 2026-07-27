//! Probe the SimulatorKit framebuffer capture path against a booted simulator.
//!
//! Run with: `cargo run -p accessibility-ios-sys --example framebuffer_probe`

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("framebuffer_probe only runs on macOS");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use accessibility_ios_sys::SimFramebuffer;
    use std::time::{Duration, Instant};

    let mut fb = SimFramebuffer::new(None)?;
    println!("device udid: {}", fb.device_udid());

    fb.start()?;

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = 0u64;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        let stats = fb.stats();
        let delta = stats.frame_count - last;
        last = stats.frame_count;
        println!(
            "frames={} (+{}) size={}x{} descriptors={} rewires={}",
            stats.frame_count,
            delta,
            stats.width,
            stats.height,
            stats.descriptor_count,
            stats.rewire_count,
        );
    }

    let stats = fb.stats();
    fb.stop();

    if stats.frame_count == 0 {
        anyhow::bail!("no frames captured - the framebuffer pipeline did not come up");
    }
    println!(
        "OK: captured {} frames at {}x{}",
        stats.frame_count, stats.width, stats.height
    );
    Ok(())
}
