# accessibility-cli

Cross-platform accessibility tree reading, querying, screenshots, and input automation for macOS, Windows, Linux, iOS Simulator, and Android.

This repository contains:

- `accessibility-core`: reusable Rust library exposing the high-level `App`/`Locator` API and platform accessibility adapters.
- `accessibility-cli`: primary `accessibility-cli` command-line interface plus a public runner entrypoint for compatibility wrappers.

The CLI preserves the existing operational surface from the SkyVM guest tooling while making the accessibility implementation reusable outside SkyVM.

On macOS, PID-targeted keyboard input, pixel clicks, and window screenshots use per-process CoreGraphics/SkyLight paths when available. This lets automation act on a target app without moving the shared cursor or capturing unrelated occluding windows.
