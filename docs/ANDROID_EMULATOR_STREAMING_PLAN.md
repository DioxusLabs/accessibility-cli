# Android Emulator Streaming Plan

## Goal

Add live Android Emulator display streaming, input, and accessibility inspection through the same encoded-frame and serving abstractions used by the iOS Simulator backend. The first implementation targets the official Android Emulator and arbitrary unmodified user applications. Physical Android devices and the Devin webapp UI are deferred.

## Chosen approach

Use a hybrid of Android's built-in interfaces:

- `adb exec-out screenrecord --output-format=h264 --time-limit 0 -` for display video;
- `EmulatorController` gRPC for persistent input and device control;
- ADB and `uiautomator` for accessibility.

`screenrecord` encodes the composed display inside Android and emits Annex-B H.264 without host-side decoding or re-encoding. Rust will split the byte stream into access units, classify keyframes, cache SPS and PPS NAL units, and publish frames through the existing `VideoCapture` interface.

```text
Android composed display
        |
        v
screenrecord / MediaCodec
        |
        | ADB exec-out, Annex-B H.264
        v
Rust access-unit parser
        |
        v
VideoCapture / session broadcast
        |
        +-- WebRTC serving
        +-- raw H.264 WebSocket serving

Browser input
        |
        v
EmulatorController streaming gRPC
        |
        v
Android virtual input devices
```

This design does not inject code into the foreground application. Capture and input operate at the emulator or Android system level, so applications do not require modification or instrumentation.

## Compatibility

The requirement applies to the Android Emulator package, not the Android API level of the virtual device.

- Initial supported emulator floor: Android Emulator 35.1.21, September 2024.
- Initial supported guest floor: Android 7.0 / API 24, where raw H.264 `screenrecord` output is available.
- Recommended version: latest stable Android Emulator.
- Runtime behavior: detect the required gRPC services and `screenrecord` options rather than trusting version strings alone.

Virtual devices may use any Android system image at or above the guest floor that is supported by the installed emulator.

## Phase 1: discovery and capability detection

Implement host-side discovery in `accessibility-android-sys`:

1. Locate active emulator discovery files:
   - Linux: `~/.android/avd/running/pid_*.ini`
   - macOS: `~/Library/Android/avd/running/pid_*.ini`
2. Parse the gRPC port, token, AVD identity, and related metadata.
3. Resolve an explicitly requested emulator when possible.
4. Require one unambiguous running emulator when no identifier is supplied.
5. Connect to `EmulatorController` and `android.emulation.control.v2.Rtc`.
6. Return actionable errors for missing discovery data, authentication failure, unsupported services, and ambiguous selection.

Use minimal protocol definitions for only the services and messages this project needs. Keep the definitions versioned with the implementation and test their wire-facing serialization.

## Phase 2: measured RTC result and video probe

The RTC experiment was implemented as:

```sh
cargo run -p accessibility-android-sys --example emulator_webrtc_probe
```

Measured on the standard macOS SDK Android Emulator 37.1.11:

1. Emulator discovery, bearer authentication, and `EmulatorController.getStatus` succeeded.
2. Android Studio's default JWT allowlist rejected `Rtc.RequestRtcStream` because the RTC service is not listed.
3. A separate emulator launched with explicit insecure `-grpc 8556`, matching Google's container launcher, removed the authentication restriction.
4. `Rtc.RequestRtcStream` then returned gRPC `UNIMPLEMENTED`.
5. The installed macOS emulator package contains no WebRTC module or video bridge executable.

The RTC v2 definitions exist in the emulator source tree but are not a portable capability of standard SDK emulator packages. Version checks alone cannot make this backend reliable. The implementation therefore uses raw H.264 `screenrecord` video with gRPC input.

The capture probe must now verify:

1. `screenrecord --output-format=h264 --time-limit 0 -` produces a sustained byte stream.
2. The parser handles three- and four-byte Annex-B start codes across arbitrary read boundaries.
3. NAL units are grouped into complete access units using access-unit delimiters, slice headers, timestamps when available, and encoder behavior observed from supported emulator builds.
4. SPS and PPS are cached and prepended to keyframes.
5. New clients and lag recovery restart `screenrecord` to force a new encoder session and decoder entry point.
6. Capture restart latency is measured and bounded.
7. Size, bitrate, frame rate, geometry, and long-running behavior are reported.

The supported Android 36 image reports screenrecord v1.4, accepts `--time-limit 0`, and successfully emitted raw H.264 through `adb exec-out`.

## Phase 3: reusable Android emulator session

Implement an Android emulator session in `accessibility-core` with the same responsibilities as the iOS simulator session:

- own the capture, input, and accessibility resources for one emulator;
- publish encoded frames through a small bounded broadcast channel;
- request an immediate keyframe for new subscribers;
- track lag and request recovery after dropped frames;
- expose raw and encoded geometry;
- collect common stream statistics;
- keep input independent from slow accessibility tree operations;
- stop and release the `screenrecord` child and gRPC streams deterministically.

Implement `AndroidEmulatorVideoCapture` as `VideoCapture`:

- `geometry()` returns source display geometry;
- `encoded_geometry()` reflects the configured screenrecord size;
- `request_keyframe()` replaces the screenrecord process with a fresh encoder session;
- `stop()` terminates capture and joins the parser worker;
- recording remains unsupported until a separate recording design is chosen.

Move genuinely shared stream statistics and browser-facing accessibility response types out of the iOS-only module rather than duplicating their definitions.

## Phase 4: input

Use `EmulatorController.streamInputEvent` or the corresponding direct RPCs for:

- touch begin, move, and end;
- multiple pointer identifiers where supported;
- keyboard down and up;
- UTF-8 text;
- scrolling and wheel events;
- Home, Back, Recents, Power, and other Android hardware keys;
- orientation changes.

Normalize coordinates at the session boundary and map them against current oriented display geometry. Preserve ordering on a persistent stream instead of spawning an ADB process per event.

Where the existing client protocol supplies USB HID usage codes, use the emulator protocol's USB code type rather than maintaining an unnecessary second key map.

## Phase 5: accessibility inspection

Reuse `AndroidAccessibility` and its ADB client for tree collection:

1. Fetch the hierarchy through `uiautomator dump`.
2. Flatten it into the shared `ElementDetail` representation.
3. Normalize element bounds against current display geometry.
4. Preserve labels, values, resource identifiers, roles, states, and actions.
5. Compute tree coverage through the shared coverage grid.
6. Implement hit testing against the latest cached tree, preferring the smallest containing element.

Android hit testing is cache-based and must not be presented as the same authoritative live operation available through the iOS accessibility bridge. Point-grid discovery remains iOS-specific unless an Android need is demonstrated.

## Phase 6: standalone serving interface

Add a command parallel to `serve-sim`:

```sh
cargo run -p accessibility-cli -- serve-emulator --serial emulator-5554 --port 3200
```

It should support the existing video options where the emulator RTC sender can honor them and expose the same core surface:

- source configuration and geometry;
- live encoded video;
- input WebSocket;
- accessibility snapshot;
- accessibility hit test;
- orientation;
- stream statistics.

Refactor `accessibility-serve` around a platform-neutral session boundary rather than copying the HTTP, WebSocket, AVCC, and WebRTC transport implementations.

## Phase 7: tests and verification

Add tests at representation boundaries:

- discovery file parsing and emulator selection;
- missing, ambiguous, and unsupported emulator errors;
- protobuf request serialization and response decoding;
- Annex-B stream splitting across arbitrary read boundaries;
- H.264 access-unit assembly;
- SPS/PPS caching and keyframe reconstruction;
- screenrecord restart recovery;
- coordinate mapping under each orientation;
- Android tree flattening, normalization, coverage, and hit testing;
- session lag and keyframe-request accounting.

Run:

```sh
cargo fmt --all
cargo build --workspace
cargo clippy --workspace --all-targets
cargo test --workspace --lib
cargo test -p accessibility-cli --test cli_smoke
```

End-to-end verification requires a supported running emulator. A successful run must cover:

- arbitrary APK installation and launch;
- sustained video under motion;
- touch drag and scrolling;
- text and hardware keys;
- rotation and geometry updates;
- accessibility tree and hit testing;
- decoder recovery after capture restart;
- disconnect and reconnect.

Do not claim end-to-end validation if only compile and unit verification were possible.

## Validation record

Validated on macOS against Android Emulator 37.1.11 running an Android 36 arm64 image:

- raw screenrecord H.264 at 570x1280 and 1280x570;
- Constrained Baseline SPS, PPS, IDR, and delta-frame parsing;
- 56-70 frames during a five-second gesture stimulus;
- approximately 0.9-1.2 Mbps during that stimulus;
- fresh SPS/PPS+IDR approximately 170-470 ms after capture restart;
- gRPC touch injection against the Settings UI;
- ADB hardware Home fallback after the emulator's successful `sendKey` RPC produced no action;
- accessibility tree, normalized coverage, cached hover picking, and live hit testing;
- browser WebCodecs rendering through the raw H.264 WebSocket;
- exact portrait/landscape rotation using `wm fixed-to-user-rotation` and orientation-aware capture restart;
- return from landscape to portrait with encoded geometry updated in both directions.

The standard macOS emulator package did not provide RTC v2 media streaming even at Emulator 37.1.11, so RTC source presence is not used as a compatibility claim.

## Deferred integration

Do not modify `devin-webapp` until the standalone backend is validated.

The follow-up integration will:

1. update the `accessibility-core` revision used by `devin-remote`;
2. add `source=android_emulator` to the display source selection;
3. forward Android frames through the existing port 6080 writer;
4. translate the shared request/response protocol for Android input and inspection;
5. add a separate Android Emulator tab while sharing transport, inspector, and device-frame UI internals with iOS;
6. add the Android quote surface and platform availability gating.

## Explicit non-goals for the first implementation

- Physical Android devices.
- Target-app instrumentation or injection.
- A scrcpy server dependency.
- An RTC/WebRTC capture backend in standard SDK emulator packages.
- Audio streaming.
- MP4 recording.
- Multi-emulator simultaneous serving from one process.
- Devin webapp UI changes.
