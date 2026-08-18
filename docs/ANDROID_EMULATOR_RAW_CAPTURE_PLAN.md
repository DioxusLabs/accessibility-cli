# Android Emulator Raw Capture and Devin Remote Integration

## Goal

Use the Android Emulator's raw framebuffer stream as an input to `devin-remote`'s existing software H.264 encoder and congestion controller. Keep the screenrecord backend as the standalone and compatibility fallback.

The production path must preserve:

- emulator source timestamps and frame sequence numbers;
- dynamic bitrate, resolution, and frame-rate control;
- explicit keyframe requests;
- bounded queues and stale-frame dropping;
- the existing QUIC/WebSocket transport adaptation boundary;
- Android touch, keyboard, rotation, and accessibility inspection.

## Architecture

```text
Android Emulator renderer
        |
        | EmulatorController.streamScreenshot
        | RGBA8888 + sequence + timestampUs
        | normal gRPC bytes or local MMAP
        v
Android raw capture source
        |
        | Arc<RgbaImage>
        v
devin-remote desktop_stream
        |
        +-- RGBA -> I420
        +-- x264 or OpenH264
        +-- force keyframe
        +-- bitrate / pixel / fps ladder
        +-- capture and encode pacing
        v
existing StreamEvent::Frame
        |
        v
existing congestion controller
        |
        v
QUIC or WebSocket transport
```

Input remains independent:

```text
browser input
    -> devin-remote
    -> EmulatorController gRPC for touch, text, and ordinary keys
    -> ADB fallback for Android hardware buttons
```

Accessibility remains ADB and `uiautomator` through `accessibility-core`.

## Why not goldfish-webrtc-bridge

`goldfish-webrtc-bridge` is a conditionally built, emulator-bundled host executable. Newer source also exposes the implementation as the `android-webrtc` shared library. It exists to connect the emulator framebuffer to Chromium libwebrtc encoding and WebRTC transport.

It is not a separately supported SDK Manager package, and the standard macOS Emulator 37.1.11 package does not ship it. Building it requires the emulator source tree and Chromium's libwebrtc dependencies.

`devin-remote` already has the parts the bridge would add: software H.264 encoding, keyframe control, pacing, link measurements, and congestion adaptation. Only the raw emulator capture source is needed.

## Phase 1: protocol and capture probes

Extend the minimal EmulatorController protocol with:

- `streamScreenshot(ImageFormat) returns (stream Image)`;
- `ImageFormat` with RGBA8888, requested dimensions, orientation, display id, and optional transport;
- `ImageTransport` with MMAP channel and handle;
- `Image` with pixels, sequence number, and timestamp.

Add a probe that measures both modes against a running emulator:

1. RGBA8888 in ordinary gRPC messages.
2. RGBA8888 through a client-owned memory-mapped file.

For each mode report:

- negotiated dimensions and orientation;
- frame count and cadence under motion;
- capture-to-receive latency from `timestampUs`;
- sequence gaps;
- bytes copied per frame;
- MMAP tearing or inconsistent-frame observations;
- behavior across rotation and reconnect.

MMAP is accepted only if it is supported by the standard emulator package and frame ownership can be made safe before the next write. Otherwise use ordinary local gRPC at the congestion controller's requested resolution.

### Measured result

Validated against Android Emulator 37.1.11 on macOS with a Pixel 10 API 36 AVD at a 1280-pixel capture bound:

| Transport | Negotiated frame | Frames | Cadence | Mean source-to-receive latency | Sequence gaps | Detected tearing |
|---|---:|---:|---:|---:|---:|---:|
| gRPC bytes | 570x1280 RGBA | 287 / 5 s | 57.3 fps | 5.1 ms | 0 | n/a |
| MMAP | 570x1280 RGBA | 302 / 5 s | 60.2 fps | 2.1 ms | 0 | 0 |

MMAP is the production default. The capture task copies each announced frame into an owned buffer immediately and publishes it through a one-frame overwrite slot. If the encoder runs below 60 fps, obsolete raw frames are dropped instead of queueing latency.

The API treats requested dimensions as an aspect-ratio bounding box. A request for 534x1200 negotiated 534x1198, so encoder geometry is always created from the first returned frame rather than the request.

## Phase 2: reusable raw frame source

Implement an Android raw frame stream below `accessibility-core`'s platform layer:

- discover and authenticate to the selected emulator;
- request RGBA8888 at an even target size;
- return owned frames with width, height, sequence, timestamp, and orientation;
- detect dropped source frames through sequence gaps;
- copy mapped bytes before acknowledging/awaiting the next frame;
- reconnect on gRPC stream termination;
- expose current source geometry and orientation;
- keep capture independent from accessibility and input workers.

The capture source must not encode video and must not use the encoded `VideoCapture` trait. The screenrecord implementation remains separate.

## Phase 3: devin-remote encoder integration

Add Android as a software-encoded source in `desktop_stream`:

1. Resolve Android display geometry before creating the encoder.
2. Request emulator frames at the geometry selected by the existing viewport and headroom policy.
3. Wrap each owned RGBA frame as the existing `capture::Frame`.
4. Feed it through `H264Encoder` and the existing `stream_software_encoded` loop.
5. Use the emulator's `timestampUs` as the capture timestamp rather than assigning it after receipt.
6. Treat the source as damage-driven because `streamScreenshot` emits on emulator frame production.
7. On forced refresh, reuse the last owned raw frame if the emulator has not produced another one.
8. On bitrate-only congestion changes, rebuild only the encoder as today.
9. On pixel-step changes or orientation changes, restart the screenshot stream and encoder with new geometry.
10. Preserve source sequence gaps and capture latency in telemetry.

Do not forward screenrecord H.264 into the remote production path. That bypasses the existing encoder and congestion controller.

## Phase 4: display source and control protocol

Extend `DisplaySource` with `android_emulator` and an optional ADB serial.

The source must send `source_config` before media frames with:

- platform and serial;
- source geometry;
- encoded geometry;
- orientation;
- capabilities for touch, keyboard, hardware buttons, rotation, and accessibility.

Reuse the existing simulator request/response shapes where they are genuinely common:

- stats;
- accessibility snapshot;
- accessibility hit test;
- orientation;
- error responses with request ids.

Keep Android-specific input translation and coordinate semantics out of the iOS implementation.

## Phase 5: congestion and QUIC adaptation

Continue using the current `Congestion` and `Headroom` controller. Feed it transport-native QUIC observations when the QUIC leg is enabled:

- ACKed goodput;
- smoothed RTT and RTT variance;
- congestion window and in-flight bytes;
- sender queue age;
- datagram loss;
- viewer decode queue and paint delay.

The existing adaptation order remains:

1. bitrate share;
2. capture pixel steps;
3. frame-rate steps.

A keyframe request continues through `StreamerControl::Keyframe` to the existing encoder. No emulator capture restart is required for decoder recovery.

## Phase 6: verification

Unit tests:

- protobuf wire shapes;
- screenshot metadata conversion;
- sequence-gap detection;
- source timestamp conversion;
- RGBA ownership and dimensions;
- MMAP bounds and copy behavior;
- source selection parsing;
- Android geometry and orientation transitions;
- controller-triggered capture restarts at pixel-step boundaries;
- iOS and desktop source compatibility.

End-to-end probes:

- raw gRPC capture;
- MMAP capture when supported;
- x264 and OpenH264 encoding;
- forced keyframe;
- bitrate backoff and recovery;
- pixel and FPS backoff;
- portrait and landscape;
- touch, text, hardware buttons, and accessibility;
- stream reconnect;
- sustained motion and an extended idle/motion soak.

Repository verification:

```sh
# accessibility-cli
cargo fmt --all
cargo build --workspace
cargo clippy --workspace --all-targets
cargo test --workspace --lib
cargo test -p accessibility-cli --test cli_smoke

# devin-webapp/apps/devin/devin-rs
cargo fmt
cargo check -p devin-remote
cargo test -p devin-remote
```

## Implementation status

Implemented and validated:

- EmulatorController screenshot protocol, ordinary gRPC probe, and MMAP probe.
- Reusable owned RGBA stream with sequence, timestamp, rotation, bottom-up correction, and mapping cleanup.
- `devin-remote` `android_emulator` display-source selection.
- MMAP capture with ordinary gRPC frame fallback.
- A dedicated 60 fps capture task and one-frame overwrite slot so encoder backpressure drops stale frames.
- Encoder geometry derived from the first negotiated frame.
- Existing RGBA-to-I420 and x264/OpenH264 encoder path.
- Existing keyframe, bitrate, frame-rate, queue-lag, goodput, RTT, and jitter controls.
- Pixel-step and orientation changes through a controlled display reconnect.
- EmulatorController gRPC touch, scroll, text, and USB keyboard input.
- ADB Home, Back, Lock, and app-switch buttons.
- Android accessibility snapshots and cached hit testing through the existing accessibility-core ADB backend.
- Source stats and source configuration messages.
- Real-emulator tests for raw frame to IDR and full WebSocket source configuration, encoded keyframe, and accessibility snapshot.

Remaining integration work:

- Add a dedicated Android Emulator workspace panel or source picker in the webapp. The backend is reachable now through `source=android_emulator&source_id=<serial>`.
- Feed transport-native QUIC ACK, congestion-window, in-flight, and datagram-loss metrics into the existing controller when the QUIC leg exposes them. The current controller continues to use drained goodput, socket queue age, viewer RTT, and jitter.
- Update `devin-remote` to the new accessibility-cli revision after these cross-repository changes are committed, then remove its temporary pinned-protocol capture client so the shared crate is the sole implementation.
- Run the extended motion/idle soak and validate the production Linux emulator image.

## Fallback

If raw screenshot streaming is unavailable or unstable on a target emulator package, use the existing screenrecord backend for that source. The fallback has coarser congestion adaptation because bitrate, size, and keyframe recovery require restarting screenrecord, but it preserves functional streaming and control.
