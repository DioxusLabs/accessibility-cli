//! Drive the iOS Simulator input worker with single- and two-finger gestures.
//!
//! Run with:
//! `cargo run -p accessibility-core --example ios_touch_probe -- <gesture> [udid]`
//!
//! Gestures: `pinch-out`, `pinch-in`, `rotate`, `pinch-then-drag`, `tap`,
//! `swipe`, `home`, `hold` (leaves both fingers down and relies on worker
//! shutdown to lift them).
//!
//! Two-finger gestures mirror the second finger through the screen centre,
//! like Simulator.app's Option-drag.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("ios_touch_probe only runs on macOS");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use std::f64::consts::FRAC_PI_2;
    use std::sync::mpsc::Sender;
    use std::time::Duration;

    use accessibility_core::platform::ios_simulator::{
        InputCommand, TouchEdge, TouchPhase, booted_simulators, spawn_input_worker,
    };
    use anyhow::Context;

    const STEPS: usize = 30;
    const FRAME: Duration = Duration::from_millis(16);

    fn touch(input: &Sender<InputCommand>, id: u32, phase: TouchPhase, (x, y): (f64, f64)) {
        let edge = if y > 0.97 {
            TouchEdge::Bottom
        } else {
            TouchEdge::None
        };

        let _ = input.send(InputCommand::Touch {
            phase,
            x,
            y,
            edge,
            id,
        });
    }

    fn mirrored((x, y): (f64, f64)) -> (f64, f64) {
        (1.0 - x, 1.0 - y)
    }

    /// Move one finger, or both mirrored, along `path` from 0 to 1.
    fn drag(input: &Sender<InputCommand>, both: bool, path: impl Fn(f64) -> (f64, f64)) {
        touch(input, 0, TouchPhase::Begin, path(0.0));
        if both {
            touch(input, 1, TouchPhase::Begin, mirrored(path(0.0)));
        }
        for step in 1..=STEPS {
            let point = path(step as f64 / STEPS as f64);
            touch(input, 0, TouchPhase::Move, point);
            if both {
                touch(input, 1, TouchPhase::Move, mirrored(point));
            }
            std::thread::sleep(FRAME);
        }
        std::thread::sleep(Duration::from_millis(150));
        let end = path(1.0);
        touch(input, 0, TouchPhase::End, end);
        if both {
            touch(input, 1, TouchPhase::End, mirrored(end));
        }
    }

    let mut args = std::env::args().skip(1);
    let gesture = args.next().context("missing gesture")?;
    let udid = match args.next() {
        Some(udid) => udid,
        None => {
            booted_simulators()?
                .into_iter()
                .next()
                .context("no booted simulator")?
                .udid
        }
    };

    let (input, capabilities) = spawn_input_worker(&udid)?;
    println!("device      : {udid}");
    println!("capabilities: {capabilities:?}");

    // Portrait iPhone framebuffers are about twice as tall as wide, so a
    // circle in pixels is half as tall in normalized y.
    let circle = |radius: f64, angle: f64| {
        (
            0.5 - radius * angle.cos(),
            0.5 - radius * 0.46 * angle.sin(),
        )
    };

    match gesture.as_str() {
        "pinch-out" => drag(&input, true, |t| circle(0.05 + 0.3 * t, FRAC_PI_2 / 2.0)),
        "pinch-in" => drag(&input, true, |t| circle(0.35 - 0.3 * t, FRAC_PI_2 / 2.0)),
        "rotate" => drag(&input, true, |t| circle(0.3, FRAC_PI_2 * t)),
        "pinch-then-drag" => {
            // Pinch out, lift the anchor finger, then keep dragging the
            // mirrored one on its own.
            let anchor = |t: f64| circle(0.05 + 0.15 * t, 0.0);
            touch(&input, 0, TouchPhase::Begin, anchor(0.0));
            touch(&input, 1, TouchPhase::Begin, mirrored(anchor(0.0)));
            for step in 1..=STEPS {
                let point = anchor(step as f64 / STEPS as f64);
                touch(&input, 0, TouchPhase::Move, point);
                touch(&input, 1, TouchPhase::Move, mirrored(point));
                std::thread::sleep(FRAME);
            }
            touch(&input, 0, TouchPhase::End, anchor(1.0));
            let start = mirrored(anchor(1.0));
            for step in 1..=STEPS {
                let t = step as f64 / STEPS as f64;
                touch(
                    &input,
                    1,
                    TouchPhase::Move,
                    (start.0 - 0.3 * t, start.1 + 0.2 * t),
                );
                std::thread::sleep(FRAME);
            }
            std::thread::sleep(Duration::from_millis(150));
            touch(&input, 1, TouchPhase::End, (start.0 - 0.3, start.1 + 0.2));
        }
        "tap" => {
            let x = args.next().map_or(Ok(0.5), |x| x.parse())?;
            let y = args.next().map_or(Ok(0.5), |y| y.parse())?;
            touch(&input, 0, TouchPhase::Begin, (x, y));
            std::thread::sleep(Duration::from_millis(50));
            touch(&input, 0, TouchPhase::End, (x, y));
        }
        "swipe" => drag(&input, false, |t| (0.8 - 0.6 * t, 0.5)),
        "home" => drag(&input, false, |t| (0.5, 0.99 - 0.5 * t)),
        "hold" => {
            touch(&input, 0, TouchPhase::Begin, circle(0.2, 0.0));
            touch(&input, 1, TouchPhase::Begin, mirrored(circle(0.2, 0.0)));
        }
        other => anyhow::bail!("unknown gesture {other}"),
    }

    // Dropping the sender shuts the worker down once it has drained.
    drop(input);
    std::thread::sleep(Duration::from_millis(500));
    Ok(())
}
