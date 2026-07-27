# serve-sim backlog

Gaps found after the first working version. Ordered by how soon they bite,
not by effort. Stream quality and performance are being worked separately.

## Broken

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

## Known limitations, probably fine

- Landscape left vs right cannot be detected, only landscape vs portrait, so
  external rotation resolves to `landscape_left` until corrected.
- `simctl ui` only implements three settings; the rest need an in-simulator
  helper.
- Single device per server. serve-sim offers a grid.
- Single-touch only; no pinch or two-finger gestures.
- No auth on the input socket. Loopback by default, so `--bind 0.0.0.0`
  currently exposes an unauthenticated control channel.

## Done since this list was written

- **Text input.** Was typing the wrong characters entirely: Indigo takes USB
  HID usages, not HIToolbox keycodes. Fixed, with modifiers, and verified by
  typing `Test@Example.com?` verbatim on device.
- **Web content discovery.** `?scan=true` sweeps what the tree walk cannot
  explain. A Safari page goes from 5 elements at 4% coverage to 15 at 50%.
- **Frame copy and display selection.** `VTPixelTransferSession` replaced the
  memcpy; the main display is chosen by `displayClass`.

## Verified working, do not re-litigate

- Multi-viewer including late join: a second viewer joining mid-stream gets
  its own parameter set and an immediate keyframe.
- Idle cost with zero viewers: 0.4% of a core idle, ~8% while animating.
  Gating encode on subscriber count is not worth it.
- A plain GPU blit is slower than `memcpy` for a bare copy. The pixel
  transfer that replaced it wins by doing the conversion and scale too.
- Constant-quality rate control is ignored *while low-latency rate control is
  enabled*. With it disabled the knob works and spans a 16x bitrate range,
  which is what the `recording` tuning is for.
