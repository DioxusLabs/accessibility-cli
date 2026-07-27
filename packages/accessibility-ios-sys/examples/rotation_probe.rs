//! Determine whether the framebuffer reflects device orientation.
//!
//! Run with: `cargo run -p accessibility-ios-sys --example rotation_probe`

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("rotation_probe only runs on macOS");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use accessibility_ios_sys::{Orientation, SimFramebuffer, SimulatorHID};
    use std::time::Duration;

    let mut framebuffer = SimFramebuffer::new(None)?;
    framebuffer.start()?;
    let hid = SimulatorHID::for_device(None)?;

    let settle = |label: &str, framebuffer: &SimFramebuffer| {
        std::thread::sleep(Duration::from_millis(2500));
        let stats = framebuffer.stats();
        println!("{label:<22} framebuffer = {}x{}", stats.width, stats.height);
    };

    settle("initial", &framebuffer);

    for (label, orientation) in [
        ("landscape_left", Orientation::LandscapeLeft),
        ("landscape_right", Orientation::LandscapeRight),
        ("portrait_upside_down", Orientation::PortraitUpsideDown),
        ("portrait", Orientation::Portrait),
    ] {
        hid.set_orientation(orientation)?;
        settle(label, &framebuffer);
    }

    framebuffer.stop();
    Ok(())
}
