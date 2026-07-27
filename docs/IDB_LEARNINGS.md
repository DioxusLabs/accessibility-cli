# What idb does that we should consider

From reading Meta's idb at `~/Dev/idb`. Accessibility findings are in
`WEB_CONTENT_ACCESSIBILITY_PLAN.md`; input bugs are in
`SERVE_SIM_BACKLOG.md`. This file covers everything else, roughly in
descending order of value.

Claims below were spot-checked against the source. Anything not verified is
marked as such.

## Video capture

### Pick the main display by `displayClass`, not by area

We choose the framebuffer descriptor with the largest live surface. idb reads
the descriptor's `state` and takes `displayClass == 0`, falling back to the
first renderable display (`FBFramebufferSurface.swift:101-129`):

```swift
// iOS exposes the main display as displayClass 0. tvOS renders only on the TVOut display (a
// non-zero class), so prefer class 0 but fall back to the first renderable display rather than
// throwing — otherwise screenshots and video are impossible on a target with no class-0 display.
```

Largest-area happens to work on iPhone but is a heuristic standing in for a
value the API actually reports, and it would pick wrong on tvOS.

They also require the descriptor to conform to both
`SimDisplayIOSurfaceRenderable` and `SimDisplayRenderable`, and try
`framebufferSurface` before falling back to the older `ioSurface` — worth
copying for older Xcodes.

### Replace the CPU memcpy with `VTPixelTransferSession`

The most valuable idea here. We `memcpy` every frame to escape SimulatorKit's
recycled IOSurface, then hand BGRA to VideoToolbox and let it convert and
scale internally. idb does all three jobs in one GPU pass
(`FBSimulatorVideoStream.swift:588-599, 678-691`): `VTPixelTransferSession`
transfers the live BGRA surface into a pooled **NV12** buffer at the target
size.

That single call replaces our copy, the implicit BGRA→NV12 conversion, and the
downscale we now want anyway. Note this does not contradict the earlier
finding that a plain Metal blit was slower than `memcpy` — that measured a
pure copy doing one job, against a purpose-built transfer doing three.

They additionally lock the buffer read-only around the encode and compare
`IOSurfaceGetSeed` before and after, counting torn frames rather than
preventing them (`FBSimulatorVideoStream.swift:729-752`). Cheap diagnostic,
worth having.

### Consider quality-based rate control instead of a bitrate

We target a bitrate derived from resolution. idb defaults to
`kVTCompressionPropertyKey_Quality` at `0.75`, with the CLI overriding to
`0.2` (`FBVideoStreamConfiguration.swift:83`, `idb/cli/commands/video.py:78`).

Constant quality sidesteps the failure mode fixed earlier — where too low a
bitrate made VideoToolbox drop frames — because there is no per-frame budget
to exceed. Worth measuring against the current setup.

### Coalesce damage events, and apply backpressure

We encode on every framebuffer callback. idb separates damage notifications
from encode pushes and offers two cadences: lazy/VFR driven by damage with an
`AsyncStream` set to `bufferingNewest(1)` so stale triggers are dropped, or
eager/CFR on a drift-corrected clock. It also refuses to push when a consumer
has more than two unprocessed frames in flight
(`FBVideoStreamWriters.swift:16-32`).

### Formats we do not have

H.264 and HEVC, over Annex-B, MPEG-TS or fragmented MP4, plus MJPEG, minicap
and raw BGRA. fMP4 in particular would make the stream playable by a plain
`<video>` via Media Source Extensions, without WebRTC or WebCodecs.

## Input

Full detail in the backlog. In brief: USB HID usage codes rather than
HIToolbox, modifiers as held key events, and a second XPC transport coming for
Xcode 27.

Two smaller things worth stealing:

- **Swipe sampling** (`FBSimulatorHIDEvent.swift:158-185`): interpolate at a
  10-point default spacing, emit repeated `.down` events, and send a
  **duplicate final down** before the up — explicitly to avoid inertial scroll
  on Apple Silicon simulators. Our scroll synthesis does not do this.
- **Two-finger and pinch** come free from `IndigoHIDMessageForMouseNSEvent` by
  passing a non-null second point, which switches the message to a
  three-payload multi-touch layout. We pass null and so cannot pinch.

### One place we are ahead

idb has no screen-edge parameter at all, so it cannot flag a touch as a system
edge gesture; a swipe-up-to-home is just a coordinate path for them. We pass
the edge as the seventh argument to `IndigoHIDMessageForMouseNSEvent`, which
is why our swipe-to-home works reliably.

## Capabilities we do not have at all

From `proto/idb.proto` and `idb/cli/commands/`: media (`photos`, `media`),
logs (`log`), app lifecycle (`app`, `launch`, `kill`), TCC
(`approve`, `revoke`), `location`, `contacts`, `keychain`, `crash`, `dap`,
`debugserver`, `dsym`, `dylib`, `file`, `focus`, `framework`, `instruments`,
`memory`, `notification`, `screenshot`, `settings`, `shell`, `xctest`.

The ones that fit a browser-based simulator tool, in rough order of value:
log streaming, media add, TCC approval, app launch/terminate, and
`notification` for push payloads.

## Direction of travel

An Objective-C to Swift migration is underway across the HID and video
subsystems, `FBFuture` is being replaced by async/await, and the newest work
is the DTUHID transport for Xcode 27. The relevant signal for us is that Apple
is moving simulator input from Indigo to a CoreDevice XPC service, and idb is
following.

Not verified: a subagent produced a list of recent video-stream commits from
web search rather than from `git log`, so those specific hashes and messages
should be re-checked locally before being relied on.
