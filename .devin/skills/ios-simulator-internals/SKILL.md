---
name: ios-simulator-internals
description: CoreSimulator and SimulatorKit private API mechanics — remote proxies, blocks, framebuffer port discovery and IOSurface lifetime. Use when touching accessibility-ios-sys or debugging why a private call silently does nothing.
triggers:
  - user
  - model
---

How to talk to the simulator's private frameworks from Rust, in
`packages/accessibility-ios-sys`. Every note here cost real debugging time and
none of it is obvious from the outside. The common thread is that these APIs
tend to **fail silently** rather than error, so the symptom is usually "nothing
happens" rather than a crash.

Simulator work needs Xcode, not just Command Line Tools:

```sh
export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
xcrun simctl list devices booted
```

`xcode-select -p` commonly points at `/Library/Developer/CommandLineTools`,
which has no simulator frameworks, so `DEVELOPER_DIR` matters.

## CoreSimulator hands back proxies

IO ports and display descriptors are `ROCKRemoteProxy` objects that implement
their interface through forwarding. `objc2::msg_send!` panics on them in debug
builds because its verification looks the selector up with
`class_getInstanceMethod`, which a forwarding proxy does not answer.

Use the helpers in `macos/dynamic.rs`, always guarded by `responds_to`.

## Blocks need a type signature

ROCKit marshals block arguments across the proxy boundary by reading the
block's ObjC type encoding, which requires `BLOCK_HAS_SIGNATURE`. `block2` does
not emit that flag (there is a TODO in its `global.rs`), so passing an
`RcBlock` aborts with "Block is missing signature field".

`macos/blocks.m` creates the blocks with clang instead. See
`macos/void_block.rs`.

## Unregistering does not stop callbacks immediately

`unregisterScreenCallbacksWithUUID:` returns while the render server may still
have a frame callback queued on the callback queue, and SimulatorKit keeps its
own reference to the block. The block therefore owns the closure it invokes
(freed from the block's dispose, not from the Rust drop). Freeing the closure
at unregister time crashed in `__invoking___` on descriptor rebuilds.

## Registering screen callbacks is load-bearing

Registration is what makes SimulatorKit attach the display pipeline and
populate `framebufferSurface`. Reading the property without registering does
not reliably work.

## Picking the right framebuffer

Several ports share `com.apple.framebuffer.display` — the main screen plus
secondary planes. Register on all of them, then pick the descriptor whose
state reports `displayClass == 0`, falling back to the largest live surface.

A booted iPhone exposes two descriptors, classes 0 and 1. Largest-area happens
to pick correctly but is a heuristic standing in for a value the API actually
reports, and it would choose wrong on tvOS, which renders on a non-zero class.

## The framebuffer IOSurface is recycled in place

Retaining the `CVPixelBuffer` does not help, because the surface mutates
underneath it. The sink must finish with a frame, or copy it, before
returning. The encoder's pixel transfer is what does that copy.

## Indigo messages are typed, not poked

The HID message layouts live in `macos/indigo.rs` as `#[repr(C, packed(4))]`
structs with `const` asserts on every offset the code depends on. A touch
builder's buffer is copied into one of them and freed in the same call
(`take_builder_message`), then edited by field and sent by pointer with
`freeWhenDone:NO`; button and keyboard messages go straight from the builder
to the client with `freeWhenDone:YES`. Either way the caller blocks until the
completion block runs. Do not add `ptr::add(0x..)` offset arithmetic back —
add a field to the struct.

Measured on Xcode 26.6: the builder leaves the mach header's `size` at zero,
so the header's payload stride and count are the only size information; both
the one-point and two-point messages use a `0xa0` stride with two and three
payloads; and the edge flags depend only on the edge (`0x3`, then
`0x2040003`/`0x8040003`/`0x1040003`/`0x4040003` for left/top/bottom/right),
which is why `SimulatorHID` caches them per edge.

```sh
cargo run -p accessibility-ios-sys --example indigo_layout_probe
```

dumps what the installed SimulatorKit actually returns and fails if the
two-point layout or the per-edge flags differ from what `indigo.rs` assumes.

## SimulatorKit moved in Xcode 27

From `Developer/Library/PrivateFrameworks` to `Contents/SharedFrameworks`.
Both are probed.

## Verifying without a browser

```sh
cargo run -p accessibility-ios-sys --example framebuffer_probe
```

Fails loudly if no frames arrive, if no keyframe is produced, or if any access
unit is not Annex-B framed.
