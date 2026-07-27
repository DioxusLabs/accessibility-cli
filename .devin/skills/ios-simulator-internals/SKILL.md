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

`macos/blocks.c` creates the blocks with clang instead. See
`macos/void_block.rs`.

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

## SimulatorKit moved in Xcode 27

From `Developer/Library/PrivateFrameworks` to `Contents/SharedFrameworks`.
Both are probed.

## Verifying without a browser

```sh
cargo run -p accessibility-ios-sys --example framebuffer_probe
```

Fails loudly if no frames arrive, if no keyframe is produced, or if any access
unit is not Annex-B framed.
