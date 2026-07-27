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

## Reaching web content

`get_tree` walks the frontmost app and so cannot see anything in another
process; a Safari page reports five elements of chrome. Hit testing *does*
cross that boundary, so `?scan=true` marks everything the tree walk explained
on a coverage grid and then probes the cells left over. On a real page that
takes 5 elements at 4% coverage to 15 at 50%, using 114 probes in under half a
second.

Swept elements are tagged `point_grid` rather than `recursive`: they are point
samples with no parent, no children and no document order.

## Accessibility hit testing

`objectAtPoint:` returns a platform element whose *own* translation must also
be given the bridge delegate token — it is not necessarily the translation you
tokenized on the way in. Miss that and attribute reads do not fail, they
silently return an empty label and a zero frame, which downstream looks like
"this element cannot be selected" rather than like an error. `get_tree` already
does this; see the matching step in `get_element_at_point`.

Two more things worth knowing before debugging the picker:

- Every app has full-screen backdrops (the Application node plus one or more
  container groups). Hit testing empty space resolves to one, and highlighting
  it paints over the whole device. They are filtered out rather than drawn.
- The tree is app-scoped and the hit test is display-scoped, so the status bar
  appears in hit tests but never in `get_tree`.
- Web content is not in the tree, but hit testing *does* reach it.
  `objectAtPoint:` resolves elements inside a `WKWebView` while `get_tree`
  returns only the host app's chrome, and there is no hierarchy to traverse
  from either end. Applies to Safari and to any embedded web view.
  See `docs/WEB_CONTENT_ACCESSIBILITY_PLAN.md`.

## HID keycodes

`IndigoHIDMessageForKeyboardArbitrary` takes **USB HID usage codes**, not
HIToolbox virtual keycodes — measured, by sending values and reading back what
appeared in a text field. `a`-`z` are `4`-`29`, Left Shift is `225`. idb's own
comment claiming HIToolbox is wrong.

Modifiers are ordinary key events held around the target key, so shifted
characters are Shift-down, key-down, key-up, Shift-up.

On Xcode 27 / CoreSimulator 1155.4+ an active `dtuhidd` silently disables
legacy Indigo keyboard events; they are delivered correctly and produce no
text. See `docs/IDB_LEARNINGS.md`.

## Device settings

`simctl ui` only implements `appearance`, `increase_contrast` and
`content_size`. The other options in Xcode's Devices window (reduce motion,
colour filters, transparency, VoiceOver) have no simctl verb and need a helper
binary spawned inside the simulator that drives the private libAccessibility
setters.

## Stream quality

`GET /api/stats` reports frames, fps, bitrate, mean frame size, keyframe
requests, lag events and — the number that matters — **bits per pixel** against
the *encoded* resolution. Use it before changing anything here.

Rules of thumb for screen content:

    < 0.05 bpp    heavy blocking, and the encoder starts dropping frames
    0.10-0.20     what to aim for
    > 0.30        wasted bandwidth

**Starving the encoder costs frame rate, not just quality.** With low-latency
rate control VideoToolbox drops frames to stay inside its per-frame budget,
which is `AverageBitRate / ExpectedFrameRate` regardless of the rate actually
being achieved. Measured on a 1206x2622 device during scrolling:

    6 Mbps    12.2 KB/frame   0.0315 bpp   33.9 fps
    24 Mbps   32.2 KB/frame   0.0835 bpp   60.3 fps

So "chunky, slow and janky" was a single root cause, not three.

The fix is resolution, not bitrate. A phone framebuffer is roughly fifteen
times the pixels the browser actually displays, so the long edge is capped at
1280 by default (`--max-dimension`, or `--native-resolution` to disable) and
the bitrate is derived from the encode resolution at ~0.15 bpp rather than
being a fixed number. Same stimulus, same bandwidth:

    native 3.16 MP @ 6 Mbps   4.99 Mbps   0.0297 bpp
    588x1280 @ derived        4.95 Mbps   0.1338 bpp

Raise `--max-dimension` if viewing in a large or retina window; the default
trades sharpness for bits on the assumption of a normal-sized preview.

`MaxKeyFrameInterval` counts frames, so on its own a "2 second" interval
stretches to twenty when the device is idle at 5fps. `MaxKeyFrameIntervalDuration`
is what actually bounds it in time; both are set.

### The frame copy is a pixel transfer, not a memcpy

Three things must happen between the framebuffer and the encoder, and
`VTPixelTransferSession` does all three in one hardware pass into a pooled
buffer: copy off the recycled live surface, convert BGRA to NV12, and
downscale.

Note this does not contradict the earlier finding that a plain Metal blit was
*slower* than `memcpy` (0.517 ms against 0.377 ms on a cache-cold 12.1 MB
surface). That measured a bare copy doing one job against a purpose-built
transfer doing three; the transfer also removes the BGRA-to-NV12 conversion
that `VTCompressionSession` would otherwise do internally.

### Latency and quality are one choice, not two knobs

`kVTCompressionPropertyKey_Quality` is **ignored** while
`EnableLowLatencyRateControl` is set. Measured with it on, quality 1.0 gave
0.0221 bpp and quality 0.4 gave 0.0223 — no effect whatsoever. Measured with
it off, quality 0.3 gave 0.70 Mbps and quality 0.9 gave 11.82 Mbps, a
sixteenfold range.

So the two settings are mutually exclusive, and `Tuning` pairs them into a
single choice rather than letting the useless combination be expressed:

- `Interactive { bitrate }` — low-latency rate control and `MaxFrameDelayCount`
  0, spending a bitrate derived from the encode resolution. Omitting
  low-latency costs roughly 300ms of decoder buffering, so this is what any
  live viewer wants.
- `Recording { quality }` — no low-latency constraint, so the quality target is
  honoured and bits go where the picture needs them. Latency is unbounded in
  principle.

Frames stay in decode order in both: B-frames would help recording quality,
but WebRTC's payloader and the raw stream framing both assume output order
matches input order.

### Main display selection

Pick the descriptor whose state reports `displayClass == 0`, falling back to
the largest live surface. A booted iPhone exposes two framebuffer descriptors,
classes 0 and 1; largest-area happens to pick correctly but is a heuristic
standing in for a value the API actually reports.

## Browser video gotcha

Do not pass `hardwareAcceleration: "prefer-hardware"` to `VideoDecoder`.
Despite the name it is treated as a requirement, and phone-shaped resolutions
like 1206x2622 exceed what hardware decoders accept, making the configuration
unsupported outright. Also note the real WebCodecs member is
`optimizeForLatency`, not `optimizeFor`. `configure()` reports success
synchronously and only surfaces the failure through the async error callback,
so check `isConfigSupported` first.
