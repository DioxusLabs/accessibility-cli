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

There are **two** normalized spaces and they only coincide in portrait, which
makes conflating them very easy and the bug invisible until you rotate.

- **Raw framebuffer space** — what HID input uses. The framebuffer is always
  portrait-native: rotating the device rotates the *content* inside a
  fixed-size surface, so pointer coordinates must be un-rotated before
  injection.
- **Logical space** — what accessibility uses. iOS has already applied the
  rotation: in landscape the app reports its own bounds as 874x402 rather than
  402x874, so normalizing against them yields upright coordinates that need no
  further rotation.

Concretely, in the web UI a tap sends raw coordinates while a hit test sends
display coordinates, and AX rects are drawn without rotation.

Normalize AX rects with `(rect.origin - app_bounds.origin) / app_bounds.size`.
`get_screen_bounds` is only populated after a tree has been read.

## Orientation

The framebuffer never changes size, so orientation cannot be recovered from
the video. It is tracked server-side and seeded at startup from the
accessibility bounds aspect ratio, which is the only cheap signal — and it
only distinguishes landscape from portrait, not left from right.

Rotation itself is a GSEvent mach message to `PurpleWorkspacePort`, not an
Indigo event. It needs Simulator.app running, because the runtime alone does
not publish that port.

## System edge gestures

Swipe-up-to-home does not work unless the touch is flagged with the screen
edge it started from. That edge is the 7th argument to
`IndigoHIDMessageForMouseNSEvent`; on arm64 it lands in x4 while the `NSSize`
argument occupies d0/d1, so a wrong declaration can silently pass zero there
and every gesture just becomes an ordinary drag. The same edge must be
supplied for every event in the gesture, and the edge is in *raw* framebuffer
space, so it rotates with the device.

## Device settings

`simctl ui` only implements `appearance`, `increase_contrast` and
`content_size`. The other options in Xcode's Devices window (reduce motion,
colour filters, transparency, VoiceOver) have no simctl verb and need a helper
binary spawned inside the simulator that drives the private libAccessibility
setters.

## Performance

The per-frame framebuffer `memcpy` looks like an obvious optimization target
and is not. Measured on a 1206x2622 surface with cache-cold sources:

    CPU memcpy   0.377 ms/copy   33.5 GB/s
    Metal blit   0.517 ms/copy   24.5 GB/s

A GPU blit is *slower*, because command buffer submission plus the
`waitUntilCompleted` round trip costs more than the copy saves on unified
memory. At 60fps the copy is ~23 ms/s, about 2% of one core.

## Browser video gotcha

Do not pass `hardwareAcceleration: "prefer-hardware"` to `VideoDecoder`.
Despite the name it is treated as a requirement, and phone-shaped resolutions
like 1206x2622 exceed what hardware decoders accept, making the configuration
unsupported outright. Also note the real WebCodecs member is
`optimizeForLatency`, not `optimizeFor`. `configure()` reports success
synchronously and only surfaces the failure through the async error callback,
so check `isConfigSupported` first.
