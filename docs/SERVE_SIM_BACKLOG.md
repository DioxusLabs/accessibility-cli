# serve-sim backlog

Gaps found after the first working version. Ordered by how soon they bite,
not by effort. Stream quality and performance are being worked separately.

## Broken

### Text input cannot type real text

`send_key` sends a bare down/up of a single keycode with no modifiers, and the
browser's key map has no shifted characters. Replaying `Test@Example.com?`
through the client mapping:

- dropped entirely: `@`, `?`
- silently lowercased: `T`, `E`

So it types `testexample.com`. No capitals, no `@`, no `?`, no `:` — you
cannot enter an email address or a URL with a query string. For a tool meant
to let an agent drive the simulator this is the most damaging gap.

Two fixes, preferring the second:

- Add modifier support to the Indigo keyboard message plus a text-to-keystroke
  mapper (serve-sim does this in `text-to-keys.ts`).
- **`simctl pbcopy` then Cmd+V.** Handles unicode and emoji, sidesteps
  keyboard layout entirely. Keep per-key input for navigation and use paste
  for text.

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

### Safari web content is not in the accessibility tree

Not a picker bug and not fixable from here. Web content lives in a separate
WebContent process and `AXPTranslator` only returns Safari's own toolbar, so a
web page shows six elements no matter how rich it is. See
`docs/ios-safari-web-content-accessibility.md`; idb reaches it through
SimulatorBridge over Distributed Objects, which is a separate mechanism from
the one used here. Test the picker against native apps.

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
