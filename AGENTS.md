# accessibility-cli

Cross-platform accessibility tree reading, querying, and automation, plus a
live iOS Simulator stream.

## Build and verify

```sh
cargo build --workspace
cargo clippy --workspace --all-targets
cargo test --workspace --lib          # unit tests, all green
cargo test -p accessibility-cli --test cli_smoke
```

### Tests that need a permitted GUI session

`packages/accessibility-cli/tests/cli_macos.rs` drives the real macOS
accessibility API: it launches Calculator and reads its window tree. It fails
with "Accessibility permissions not granted" or "Calculator never opened a
window" unless the terminal running the tests has been granted
System Settings > Privacy & Security > Accessibility. These failures are
environmental, not regressions.

## iOS Simulator work

Anything touching the simulator needs Xcode (not just Command Line Tools) and
a booted device:

```sh
export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
xcrun simctl list devices booted
```

`xcode-select -p` on this machine points at `/Library/Developer/CommandLineTools`,
which has no simulator frameworks, so `DEVELOPER_DIR` matters.

Verify the capture and encode pipeline end to end without a browser:

```sh
cargo run -p accessibility-ios-sys --example framebuffer_probe
```

Serve it:

```sh
cargo run -p accessibility-cli -- serve-sim --port 3200
```

### Hard-won specifics live in skills

The simulator work depends on a lot of private-framework behaviour that fails
silently rather than erroring. Rather than carry all of it here, it is split
into focused skills under `.devin/skills/`, which are loaded when relevant:

| Skill | Covers |
|---|---|
| `ios-simulator-internals` | CoreSimulator and SimulatorKit mechanics: remote proxies, block signatures, framebuffer port discovery, IOSurface lifetime |
| `ios-simulator-input` | The two coordinate spaces, USB HID keycodes, system edge gestures, orientation, device settings |
| `ios-simulator-video` | Bits per pixel, VideoToolbox rate control, pixel transfer, MP4 recording, browser-side WebCodecs |
| `ios-simulator-accessibility` | Bridge delegate tokens, backdrops, app versus display scope, reaching web content |

Read the relevant one before changing that area. Each is short, and every note
in them cost real debugging time.

## Longer-form notes

- `docs/IDB_LEARNINGS.md` — what Meta's idb does differently and what is worth
  taking from it
- `docs/WEB_CONTENT_ACCESSIBILITY_PLAN.md` — why web content is invisible to
  the tree and how the point-grid sweep addresses it
- `docs/SERVE_SIM_BACKLOG.md` — known gaps, ordered by how soon they bite
