//! Dump what `IndigoHIDMessageForMouseNSEvent` returns, to check the layouts
//! `macos/indigo.rs` assumes against the installed SimulatorKit.
//!
//! Run with: `cargo run -p accessibility-ios-sys --example indigo_layout_probe`
//!
//! For each single/two-point call, event type, edge and point it prints the
//! header (mach size, payload stride, kind byte) and every payload's touch
//! fields. Exits non-zero if the two-point message does not have the
//! three-payload `0xa0`-stride layout or the edge flags vary with anything
//! other than the edge.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("indigo_layout_probe only runs on macOS");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use std::collections::BTreeMap;
    use std::ffi::c_void;

    use accessibility_ios_sys::load_simulatorkit_framework;
    use anyhow::{Context, anyhow};
    use objc2_core_foundation::CGPoint;

    type Builder = unsafe extern "C" fn(
        *const CGPoint,
        *const CGPoint,
        i32,
        i32,
        u32,
        f64,
        f64,
    ) -> *mut c_void;

    const HEADER: usize = 0x20;
    const TOUCH: usize = 0x10;

    fn u32_at(base: *const u8, offset: usize) -> u32 {
        unsafe { std::ptr::read_unaligned(base.add(offset) as *const u32) }
    }
    fn u64_at(base: *const u8, offset: usize) -> u64 {
        unsafe { std::ptr::read_unaligned(base.add(offset) as *const u64) }
    }
    fn f64_at(base: *const u8, offset: usize) -> f64 {
        unsafe { std::ptr::read_unaligned(base.add(offset) as *const f64) }
    }

    let handle = load_simulatorkit_framework()?;
    let builder: Builder = unsafe {
        let sym = libc::dlsym(handle, c"IndigoHIDMessageForMouseNSEvent".as_ptr());
        if sym.is_null() {
            return Err(anyhow!("IndigoHIDMessageForMouseNSEvent not found"));
        }
        std::mem::transmute(sym)
    };

    let points = [CGPoint { x: 0.1, y: 0.2 }, CGPoint { x: 0.9, y: 0.8 }];
    let mut failures = Vec::new();
    let mut flags_by_edge: BTreeMap<u32, Vec<(String, u32)>> = BTreeMap::new();

    for two_fingers in [false, true] {
        for event_type in [1, 2] {
            for edge in 0..=4u32 {
                for first in points {
                    let second = CGPoint {
                        x: 1.0 - first.x,
                        y: 1.0 - first.y,
                    };
                    let second_ptr = if two_fingers {
                        &second as *const CGPoint
                    } else {
                        std::ptr::null()
                    };
                    let message =
                        unsafe { builder(&first, second_ptr, 0x32, event_type, edge, 1.0, 1.0) };
                    let base = message as *const u8;
                    if base.is_null() {
                        return Err(anyhow!("builder returned null"));
                    }

                    let mach_size = u32_at(base, 0x04);
                    let stride = u32_at(base, 0x18) as usize;
                    let kind = unsafe { *base.add(0x1c) };
                    let label = format!(
                        "{} event={event_type} edge={edge} point=({:.1},{:.1})",
                        if two_fingers { "two" } else { "one" },
                        first.x,
                        first.y
                    );
                    println!("{label}: mach_size=0x{mach_size:x} stride=0x{stride:x} kind={kind}");

                    let payloads = usize::from(kind);
                    for index in 0..payloads {
                        let payload = unsafe { base.add(HEADER + index * stride) };
                        let touch = unsafe { payload.add(TOUCH) };
                        println!(
                            "  payload {index}: kind=0x{:x} timestamp={} | state={} state2={} \
                             edge_flags=0x{:x} x={:.3} y={:.3} touching={} in_range={}",
                            u32_at(payload, 0x00),
                            u64_at(payload, 0x04),
                            u32_at(touch, 0x00),
                            u32_at(touch, 0x04),
                            u32_at(touch, 0x08),
                            f64_at(touch, 0x0c),
                            f64_at(touch, 0x14),
                            u32_at(touch, 0x34),
                            u32_at(touch, 0x38),
                        );
                    }

                    if two_fingers && (stride != 0xa0 || kind != 3) {
                        failures.push(format!("{label}: expected stride 0xa0 and kind 3"));
                    }
                    if !two_fingers && payloads > 1 {
                        flags_by_edge.entry(edge).or_default().push((
                            label,
                            u32_at(unsafe { base.add(HEADER + stride + TOUCH) }, 0x08),
                        ));
                    }

                    unsafe { libc::free(message) };
                }
            }
        }
    }

    for (edge, flags) in &flags_by_edge {
        let first = flags.first().context("no samples")?.1;
        if flags.iter().any(|(_, value)| *value != first) {
            failures.push(format!(
                "edge {edge}: flags vary by point or event type: {flags:?}"
            ));
        } else {
            println!("edge {edge}: flags=0x{first:x} (consistent)");
        }
    }

    if failures.is_empty() {
        println!("layout OK");
        Ok(())
    } else {
        Err(anyhow!("{}", failures.join("\n")))
    }
}
