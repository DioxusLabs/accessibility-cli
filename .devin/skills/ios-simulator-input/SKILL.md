---
name: ios-simulator-input
description: Injecting touch, keyboard and rotation into the iOS Simulator — the two coordinate spaces, USB HID keycodes, system edge gestures and device settings. Use when working on taps, swipes, scrolling, typing, orientation or anything that drives the simulator.
triggers:
  - user
  - model
---

Everything about getting input *into* the simulator. The recurring hazard is
that wrong input is usually accepted without complaint: the event is delivered,
it just does the wrong thing or nothing at all.

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

## HID keycodes

`IndigoHIDMessageForKeyboardArbitrary` takes **USB HID usage codes**, not
HIToolbox virtual keycodes — measured, by sending values and reading back what
appeared in a text field. `a`-`z` are `4`-`29`, Left Shift is `225`. idb's own
comment claiming HIToolbox is wrong.

The two code spaces overlap in range and disagree on nearly every value, so
getting it wrong types different letters rather than failing.

Modifiers are ordinary key events held around the target key, so shifted
characters are Shift-down, key-down, key-up, Shift-up. There is no shift flag
on the Indigo message.

The character-to-key table lives in
`accessibility-core/src/platform/ios_simulator/keymap.rs` and is US-ASCII only;
unmappable characters fail the whole string rather than typing
a subtly wrong one.

On Xcode 27 / CoreSimulator 1155.4+ an active `dtuhidd` silently disables
legacy Indigo keyboard events; they are delivered correctly and produce no
text. See `docs/IDB_LEARNINGS.md`.

## System edge gestures

Swipe-up-to-home does not work unless the touch is flagged with the screen
edge it started from. That edge is the 7th argument to
`IndigoHIDMessageForMouseNSEvent`; on arm64 it lands in x4 while the `NSSize`
argument occupies d0/d1, so a wrong declaration can silently pass zero there
and every gesture just becomes an ordinary drag.

The same edge must be supplied for every event in the gesture, and the edge is
in *raw* framebuffer space, so it rotates with the device.

## Orientation

The framebuffer never changes size, so orientation cannot be recovered from
the video. It is tracked server-side and seeded at startup from the
accessibility bounds aspect ratio, which is the only cheap signal — and it
only distinguishes landscape from portrait, not left from right.

Rotation itself is a GSEvent mach message to `PurpleWorkspacePort`, not an
Indigo event. It needs Simulator.app running, because the runtime alone does
not publish that port.

## Device settings

`simctl ui` only implements `appearance`, `increase_contrast` and
`content_size`. The other options in Xcode's Devices window (reduce motion,
colour filters, transparency, VoiceOver) have no simctl verb and need a helper
binary spawned inside the simulator that drives the private libAccessibility
setters.

The direct `SimDevice` getters return raw integers. Measured on Xcode 26.6:
appearance is 1 light / 2 dark, content size is 1 through 12, and increase
contrast is **1 disabled / 2 enabled** — it is not a 0/1 boolean even though
the setter accepts a BOOL.
