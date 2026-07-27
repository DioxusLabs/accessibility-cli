# serve-sim backlog

Gaps found after the first working version. Ordered by how soon they bite,
not by effort. Stream quality and performance are being worked separately.

## Broken

### Text input types the wrong characters entirely

Worse than first thought, and now measured. Two independent bugs.

**1. The keycode space is wrong.** Our map uses HIToolbox virtual keycodes,
but `IndigoHIDMessageForKeyboardArbitrary` takes **USB HID usage codes**.
Sending `0, 11, 8` — our codes for `abc` — into Safari's address bar produced
**`he`**, which is exactly what those values mean as USB HID usages (`0` is
reserved and does nothing, `11` is `h`, `8` is `e`). So keyboard input has
never worked; it silently types different letters.

Note that idb's own comment in `FBSimulatorIndigoHID.swift:68` claims the
keycodes are "'Hardware Independent' as described in `<HIToolbox/Events.h>`"
and is wrong — their Python layer correctly uses USB HID usages.

**2. No modifiers**, so no capitals and no shifted symbols. `@`, `?` and `:`
are dropped, `T` and `E` silently lowercase.

The fix, following idb (`idb/common/hid.py:102-254`):

- Switch to USB HID usage codes: `a`-`z` are `4`-`29`, `1`-`9` are `30`-`38`,
  `0` is `39`, Return `40`, Escape `41`, Backspace `42`, Tab `43`, Space `44`.
- Model modifiers as ordinary key events held around the target key: Left
  Shift `225`, Control `224`, Option `226`, Command `227`. Order is modifiers
  down, key down, key up, modifiers up in reverse.
- Port their US-ASCII table wholesale and error on unmappable characters
  rather than dropping them silently. idb has no Unicode or emoji support and
  no layout awareness either; do not claim more than is delivered.

For arbitrary Unicode a pasteboard + Cmd-V path is still attractive, but note
that `SimDevicePasteboard` was removed after Xcode 26.2 and replaced by
`SimPasteboardPlus` (Xcode 26.6+), which is push/pull against a host
`NSPasteboard` rather than one-shot get/set. idb has headers for both and
implements neither.

### Xcode 27 will silently break keyboard input

On Xcode 27 / CoreSimulator 1155.4+, an active `dtuhidd` disconnects the
legacy `ExternalKeyboardService`. Indigo keyboard events are still delivered
byte-correctly and still produce no text. idb detects this and throws rather
than typing into the void (`FBSimulatorHIDSelection.swift:17-43`): gate on the
loaded CoreSimulator version being >= 1155.4 and on a `dtuhidd` subprocess
existing for the UDID.

Their workaround is a whole second transport, `FBSimulatorDTUHIDTransport`,
which speaks plain XPC to `com.apple.coredevice.feature.remote.hid.digitizer`,
building the connection from the simulator's Mach port via private `_4sim`
symbols resolved with `dlsym` and marking it sim-to-host with
`xpc_connection_enable_sim2host_4sim`. They are adding capabilities to it one
commit at a time, which is a fair signal of where this is all heading.

Least we should do now: detect the condition and report it, instead of
appearing to work.

## Misleading

### `--fps` does not throttle anything

It only sets `ExpectedFrameRate`, which is a rate-control hint. Capture is
event-driven and unthrottled. Measured with `--fps 10` during animation: a
median inter-frame gap of 17.2 ms, roughly 58fps.

Either implement a capture-side throttle or rename it `--encoder-fps`.

## Missing for remote use

### No downscale

Streams the native 1206x2622 with no way to reduce it. Fine on loopback,
unpleasant over a tunnel. See the stream quality work — this overlaps.

## Cheap and high value

All thin wrappers over simctl:

- `simctl openurl` — deep-link into apps instead of tapping through.
- `simctl pbcopy` / `pbpaste` — pairs with the text input fix.
- `simctl addmedia` — drag a photo or video onto the device.
- `simctl spawn log stream` — forward device logs to the browser. serve-sim
  does this specifically so agents can read them.

## Robustness

### Simulator shutdown mid-serve is untested

Expected to degrade quietly: capture goes silent, HID and AX calls fail per
request, and nothing tells the user why the screen froze. Needs a deliberate
test and a clear error.

### The inspector's accessibility tree is cached on toggle only

Never refreshed, so after navigating, hover previews are stale until the
debounced hit test corrects them.

### The cached tree is app-scoped, the hit test is display-scoped

`get_tree` walks the frontmost app, so it does not contain the status bar,
which belongs to SpringBoard. `objectAtPoint:` hit tests the whole display and
does find it. The hybrid picker papers over this — hover preview misses those
elements, the confirming hit test catches them — but a tree-only consumer will
not see them.

### Web content is missing from the tree, but hit testing does reach it

Corrects an earlier note here that called this unfixable. Measured: the
element picker works fine on web content — `objectAtPoint:` resolves links,
buttons and text inside a `WKWebView` — while `get_tree` returns only the
host app's own chrome. There is no hierarchy to walk in either direction.

So picking works and every tree-based tool is blind, which matters most for
hybrid apps (Capacitor, Cordova, React Native `WebView`) where the web view is
most of the UI. See `docs/WEB_CONTENT_ACCESSIBILITY_PLAN.md`.

## Known limitations, probably fine

- Landscape left vs right cannot be detected, only landscape vs portrait, so
  external rotation resolves to `landscape_left` until corrected.
- `simctl ui` only implements three settings; the rest need an in-simulator
  helper.
- Single device per server. serve-sim offers a grid.
- Single-touch only; no pinch or two-finger gestures.
- No auth on the input socket. Loopback by default, so `--bind 0.0.0.0`
  currently exposes an unauthenticated control channel.

## Verified working, do not re-litigate

- Multi-viewer including late join: a second viewer joining mid-stream gets
  its own parameter set and an immediate keyframe.
- Idle cost with zero viewers: 0.4% of a core idle, ~8% while animating.
  Gating encode on subscriber count is not worth it.
- The per-frame `memcpy` is not the bottleneck; a Metal blit measured slower.
  See the note in `pixel_buffer.rs`.
