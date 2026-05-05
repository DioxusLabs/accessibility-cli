# Contributing

Thanks for your interest in contributing.

## Building

```sh
cargo build --workspace
```

The workspace contains two crates:

- `accessibility-core` — the cross-platform library (`packages/accessibility-core`).
- `accessibility-cli` — the binary (`packages/accessibility-cli`).

## System dependencies

Some platforms need extra libraries or services to build and run.

### Linux

```sh
sudo apt-get install -y libdbus-1-dev libatspi2.0-dev libx11-xcb-dev
```

To run the AT-SPI end-to-end tests you also need a session bus, the AT-SPI
registry, and a GUI app to drive:

```sh
sudo apt-get install -y xvfb at-spi2-core gnome-calculator dbus-x11 libgtk-3-0
```

### macOS

The CLI uses the system Accessibility APIs, so the terminal (or whatever
process invokes the CLI) needs the **Accessibility** permission in
*System Settings → Privacy & Security → Accessibility*. macOS will prompt
the first time you run the CLI; allow it and re-run.

### Windows

End-to-end tests drive the built-in Calculator. Install via winget if it
isn't already present:

```powershell
winget install 9WZDNCRFHVN5 -s msstore
```

### Android

Install the Android platform tools and put `adb` on `PATH`. Either set
`ANDROID_HOME` to your SDK root or accept the default
`~/Library/Android/sdk` (macOS) / `~/Android/Sdk` (Linux). The Android tests
will spin up an emulator if one isn't already running.

### iOS Simulator (macOS only)

Requires Xcode and at least one booted iOS Simulator runtime. The CLI loads
private Apple frameworks from the active Xcode toolchain — set the toolchain
with `xcode-select` if you have multiple installed.

## Running tests

Workspace-wide unit and integration tests:

```sh
cargo test --workspace --lib --bins
cargo test -p accessibility-cli --test cli_smoke
```

Per-platform end-to-end tests (run on the matching host):

```sh
# macOS
cargo test -p accessibility-core --test calculator_e2e -- --nocapture --test-threads=1

# Windows
cargo test -p accessibility-core --test calculator_windows_e2e -- --nocapture --test-threads=1

# Linux (under xvfb + dbus)
cargo test -p accessibility-core --test gnome_calculator_e2e -- --nocapture --test-threads=1

# Android (with `adb` available)
cargo test -p accessibility-core --test settings_android_e2e -- --nocapture --test-threads=1
```

CI runs the matching e2e on each platform — see `.github/workflows/pr-build.yml`.

## Style

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
```

Both gate CI; please run them before sending a PR.

## Submitting changes

1. Fork the repo, create a feature branch.
2. Make changes plus tests where it makes sense.
3. Run `fmt`, `clippy`, and the relevant test suites.
4. Open a PR with a short description of the change and the platforms you
   verified on.

## License

By contributing, you agree that your contributions will be dual-licensed
under the [MIT](LICENSE-MIT) and [Apache 2.0](LICENSE-APACHE) licenses, the
same terms as the rest of the project.
