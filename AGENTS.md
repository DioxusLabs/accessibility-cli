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

It fails loudly if no frames arrive, if no keyframe is produced, or if any
access unit is not Annex-B framed.

Serve it:

```sh
cargo run -p accessibility-cli -- serve-sim --port 3200
```

## Private framework notes

These cost real debugging time; they are not obvious from the outside.

- **CoreSimulator hands back proxies.** IO ports and display descriptors are
  `ROCKRemoteProxy` objects that implement their interface through forwarding.
  `objc2::msg_send!` panics on them in debug builds because its verification
  looks the selector up with `class_getInstanceMethod`. Use the helpers in
  `macos/dynamic.rs`, always guarded by `responds_to`.

- **Blocks need a type signature.** ROCKit marshals block arguments across the
  proxy boundary by reading the block's ObjC type encoding, which requires
  `BLOCK_HAS_SIGNATURE`. `block2` does not emit that flag (there is a TODO in
  its `global.rs`), so passing an `RcBlock` aborts with "Block is missing
  signature field". `macos/blocks.c` creates the blocks with clang instead.
  See `macos/void_block.rs`.

- **Registering screen callbacks is load-bearing.** It is what makes
  SimulatorKit attach the display pipeline and populate `framebufferSurface`.
  Reading the property without registering does not reliably work.

- **Several ports share `com.apple.framebuffer.display`** (main screen plus
  secondary planes). Register on all of them and pick the largest live surface
  each frame; the first match is often a small overlay.

- **The framebuffer IOSurface is recycled in place.** Retaining the
  `CVPixelBuffer` does not help because the surface mutates underneath it, so
  frames are deep-copied before going downstream. This is the main CPU cost in
  the capture path.

- **SimulatorKit moved in Xcode 27** from `Developer/Library/PrivateFrameworks`
  to `Contents/SharedFrameworks`. Both are probed.

## Coordinate spaces

- **Input** is normalized 0..1 across the whole path, browser to HID. Nothing
  converts to points or pixels, which avoids the scale ambiguity entirely.
- **Accessibility frames** come back in macOS screen points, positioned
  wherever the Simulator window sits. Normalize against the app's own bounds
  before exposing them: `(rect.origin - app_bounds.origin) / app_bounds.size`.
  `get_screen_bounds` is only populated after a tree has been read.

## Browser video gotcha

Do not pass `hardwareAcceleration: "prefer-hardware"` to `VideoDecoder`.
Despite the name it is treated as a requirement, and phone-shaped resolutions
like 1206x2622 exceed what hardware decoders accept, making the configuration
unsupported outright. Also note the real WebCodecs member is
`optimizeForLatency`, not `optimizeFor`. `configure()` reports success
synchronously and only surfaces the failure through the async error callback,
so check `isConfigSupported` first.
