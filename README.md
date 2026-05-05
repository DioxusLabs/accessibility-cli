# accessibility-cli

Cross-platform accessibility tree reading, querying, screenshots, and input
automation for **macOS**, **Windows**, **Linux**, **iOS Simulator**, and
**Android**.

The repository contains two crates:

- [`accessibility-core`](packages/accessibility-core) — reusable Rust library
  exposing a high-level `App` / `Locator` API and platform accessibility
  adapters.
- [`accessibility-cli`](packages/accessibility-cli) — the `accessibility-cli`
  command-line interface.

## Platform support

| Platform        | Tree | Query | Screenshot | Mouse | Keyboard |
|-----------------|:----:|:-----:|:----------:|:-----:|:--------:|
| macOS           |  ✓   |   ✓   |     ✓      |   ✓   |    ✓     |
| Windows         |  ✓   |   ✓   |     ✓      |   ✓   |    ✓     |
| Linux (AT-SPI)  |  ✓   |   ✓   |     ✓      |   ✓   |    ✓     |
| iOS Simulator   |  ✓   |   ✓   |     ✓      |   ✓   |    ✓     |
| Android (adb)   |  ✓   |   ✓   |     ✓      |   ✓   |    ✓     |

On macOS, PID-targeted keyboard input, pixel clicks, and window screenshots
use per-process CoreGraphics / SkyLight paths when available, so automation
can act on a target app without moving the shared cursor or capturing
unrelated occluding windows.

## Install

Build from source:

```sh
cargo install --path packages/accessibility-cli
```

A published GitHub Releases / Homebrew / `cargo install accessibility-cli`
flow is planned — see [`OPEN_SOURCE_TODO.md`](OPEN_SOURCE_TODO.md).

## Permissions

- **macOS** — grant the invoking terminal **Accessibility** permission in
  *System Settings → Privacy & Security → Accessibility* (macOS will prompt
  on first run).
- **Linux** — needs an AT-SPI registry on the session bus. On a typical
  GNOME session this is already running.
- **Android** — `adb` must be on `PATH`. The CLI uses `uiautomator dump`,
  so the device/emulator's user must allow ADB debugging.
- **iOS Simulator** — needs Xcode and at least one booted simulator runtime.

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for full per-platform setup details.

## Quick examples

```sh
# Print the accessibility tree of a running macOS app.
accessibility-cli --platform mac --pid 12345 --llm

# Click at a pixel inside a Windows app without moving the shared cursor.
accessibility-cli --platform win --pid 12345 --mouse-click 300,240

# Annotated screenshot of an iOS Simulator app.
accessibility-cli --platform ios --udid ABC123 --annotate

# HID tap on iOS Simulator.
accessibility-cli --platform ios --hid-tap 100,200

# Query a Linux app via AT-SPI.
accessibility-cli --platform linux --pid 12345 --llm

# Press Android back button.
accessibility-cli --platform android --adb-back

# Swipe on Android.
accessibility-cli --platform android --adb-swipe 100,200,100,800
```

## Library usage

```rust,ignore
use accessibility_core::api::{App, Platform};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = App::connect(pid, Platform::MacOS).await?;

    app.locator("Button[title='5']").click().await?;
    app.locator("Button[title='+']").click().await?;
    app.locator("Button[title='3']").click().await?;
    app.locator("Button[title='=']").click().await?;

    let result = app.wait_for_locator("StaticText[value*='8']").await?;
    assert!(result.value().await?.unwrap().contains("8"));
    Ok(())
}
```

## Apple private-framework usage

Some platform backends call into Apple frameworks that are not part of the
public SDK and may break or be denied at runtime under future macOS / Xcode
releases:

- **macOS**: `SkyLight.framework` is used (via `dlopen`/`dlsym`) to post
  per-process keyboard and mouse events without moving the shared cursor.
  When the symbol is unavailable, code falls back to public CoreGraphics
  paths.
- **iOS Simulator**: `AccessibilityPlatformTranslation.framework`,
  `CoreSimulator.framework`, and the SimulatorKit Indigo HID symbols are
  loaded from the active Xcode toolchain. The simulator backend will not
  start without Xcode installed and at least one booted simulator runtime.

These backends are best treated as **experimental** — pin a known-good
Xcode / macOS combination if you depend on them in CI, and expect to
revisit them on each major OS release.

## Safety and privacy

`accessibility-cli` is a powerful automation tool. It can:

- Read the full accessibility tree of any application you have permission
  to inspect (titles, values, descriptions of every element).
- Capture window or screen screenshots.
- Inject keyboard, mouse, and touch input into target applications.

Because of this, the OS treats it like an assistive technology and gates it
behind permission prompts (macOS Accessibility, Linux AT-SPI, Android ADB
debugging). Only run it against applications and devices you control.

The CLI does **not** send telemetry, phone home, or open any network
connections of its own. It only talks to the OS-level accessibility APIs,
to ADB on Android, and to local Apple frameworks for the iOS Simulator
backend.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for build/test instructions and
per-platform requirements.

## License

Licensed under either of

- Apache License, Version 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE))
- MIT license ([`LICENSE-MIT`](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in the work by you, as defined in the
Apache-2.0 license, shall be dual-licensed as above, without any additional
terms or conditions.
