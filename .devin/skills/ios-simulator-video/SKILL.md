---
name: ios-simulator-video
description: Capturing, encoding, streaming and recording the iOS Simulator screen — bits per pixel, VideoToolbox rate control, pixel transfer, MP4 recording and browser-side WebCodecs. Use when working on stream quality, frame rate, the encoder or screen recording.
triggers:
  - user
  - model
---

The capture and encode pipeline, and how to tell whether a change to it
actually helped.

**Measure before changing anything here.** Most of the notes below exist
because something that sounded obviously right turned out to be wrong when
measured.

## Stream quality: read bits per pixel first

`GET /api/stats` reports frames, fps, bitrate, mean frame size, keyframe
requests, lag events and — the number that matters — **bits per pixel** against
the *encoded* resolution.

Rules of thumb for screen content:

    < 0.05 bpp    heavy blocking, and the encoder starts dropping frames
    0.10-0.20     what to aim for
    > 0.30        wasted bandwidth

## Starving the encoder costs frame rate, not just quality

With low-latency rate control, VideoToolbox drops frames to stay inside its
per-frame budget, which is `AverageBitRate / ExpectedFrameRate` regardless of
the rate actually being achieved. Measured on a 1206x2622 device during
scrolling:

    6 Mbps    12.2 KB/frame   0.0315 bpp   33.9 fps
    24 Mbps   32.2 KB/frame   0.0835 bpp   60.3 fps

So "chunky, slow and janky" was a single root cause, not three.

The fix is resolution, not bitrate. A phone framebuffer is roughly fifteen
times the pixels the browser actually displays, so the long edge is capped at
1280 by default (`--max-dimension`, or `--native-resolution` to disable) and
the bitrate is derived from the encode resolution at ~0.15 bpp rather than
being a fixed number. Same stimulus, same bandwidth:

    native 3.16 MP @ 6 Mbps   4.99 Mbps   0.0297 bpp
    588x1280 @ derived        4.95 Mbps   0.1338 bpp

Raise `--max-dimension` if viewing in a large or retina window; the default
trades sharpness for bits on the assumption of a normal-sized preview.

`MaxKeyFrameInterval` counts frames, so on its own a "2 second" interval
stretches to twenty when the device is idle at 5fps.
`MaxKeyFrameIntervalDuration` is what actually bounds it in time; both are set.

## The frame copy is a pixel transfer, not a memcpy

Three things must happen between the framebuffer and the encoder, and
`VTPixelTransferSession` does all three in one hardware pass into a pooled
buffer: copy off the recycled live surface, convert BGRA to NV12, and
downscale.

This does not contradict the earlier finding that a plain Metal blit was
*slower* than `memcpy` (0.517 ms against 0.377 ms on a cache-cold 12.1 MB
surface). That measured a bare copy doing one job against a purpose-built
transfer doing three; the transfer also removes the BGRA-to-NV12 conversion
that `VTCompressionSession` would otherwise do internally.

## Latency and quality are one choice, not two knobs

`kVTCompressionPropertyKey_Quality` is **ignored** while
`EnableLowLatencyRateControl` is set. Measured with it on, quality 1.0 gave
0.0221 bpp and quality 0.4 gave 0.0223 — no effect whatsoever. Measured with
it off, quality 0.3 gave 0.70 Mbps and quality 0.9 gave 11.82 Mbps, a
sixteenfold range.

So the two settings are mutually exclusive, and `Tuning` pairs them into a
single choice rather than letting the useless combination be expressed:

- `Interactive { bitrate }` — low-latency rate control and `MaxFrameDelayCount`
  0, spending a bitrate derived from the encode resolution. Omitting
  low-latency costs roughly 300ms of decoder buffering, so this is what any
  live viewer wants.
- `Recording { quality }` — no low-latency constraint, so the quality target is
  honoured and bits go where the picture needs them. Latency is unbounded in
  principle.

## Recording

`start_recording` runs a **second, independent encode** of the same frames
rather than retuning the streaming encoder. That is what lets a recording use
B-frames: the live path cannot, because WebRTC's payloader and the raw stream
framing both assume the encoder emits frames in submission order. It also lets
a recording have its own resolution and quality regardless of what the viewer
is watching.

`AVAssetWriter` does the encoding and the muxing. Feeding it pixel buffers
through an `AVAssetWriterInputPixelBufferAdaptor`, rather than encoding
ourselves and appending sample buffers, avoids having to order decode and
presentation timestamps by hand — which is precisely what B-frames complicate.

Verify a recording really is what it claims to be:

```sh
ffprobe -v error -select_streams v:0 -show_entries frame=pict_type \
  -of csv=p=0 -read_intervals "%+#120" recording.mp4
```

Expect a mix of `I`, `P` and `B`. All `I` and `P` means frame reordering
silently failed to take.

Timestamps are wall-clock elapsed at nanosecond timescale, so the variable
capture rate is recorded at the speed it actually happened. Finalizing blocks
until the writer flushes: an MP4 has no playable index until then.

## Browser-side decode

Do not pass `hardwareAcceleration: "prefer-hardware"` to `VideoDecoder`.
Despite the name it is treated as a requirement, and phone-shaped resolutions
like 1206x2622 exceed what hardware decoders accept, making the configuration
unsupported outright.

The real WebCodecs member is `optimizeForLatency`, not `optimizeFor`.

`configure()` reports success synchronously and only surfaces the failure
through the async error callback, so check `isConfigSupported` first.
