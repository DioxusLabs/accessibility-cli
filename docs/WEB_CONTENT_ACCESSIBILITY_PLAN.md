# Reaching web content in the accessibility tree

Plan for exposing `WKWebView` and Safari content, which the ordinary tree walk
cannot see.

Settled by reading idb's source rather than guessing: Meta hit this exact
problem, and the approach they landed on is grid-based hit testing. Details and
citations below.

## What was measured here

Against a booted iPhone 17 running a probe page with a heading, link, button,
text field and paragraph.

**Hit testing already reaches web content.** `objectAtPoint:` resolved every
element on the page:

    Unknown   AX Probe Heading
    Link      Probe Link Alpha
    Button    Probe Button Bravo
    TextInput (no label)
    Label     Probe paragraph text content.

**The tree walk does not.** `get_tree` on the same page returns five elements,
all Safari's own chrome: Back, Page Menu, Address, refresh, More.

**There is no hierarchy to traverse, in either direction.** Building a subtree
from a hit-test result gives one node at any depth. Climbing
`accessibilityParent` from the link reaches an `AXGroup` covering the web area
(438x714) — and that group reports `children = 0`. So the content is
individually addressable but not enumerable.

**Hit tests are cheap**: 3.2 ms median, 4.0 ms p90, measured over HTTP
including the round trip.

## How this affects WebView apps

Identically, and this matters more than the Safari case.

Safari's content area *is* a `WKWebView` backed by a separate WebContent
process. An app embedding `WKWebView` gets the same architecture — there is no
in-process mode. So the measurements above apply unchanged to React Native
`WebView`, Capacitor and Cordova apps, in-app browsers,
`SFSafariViewController`, and any embedded web view.

| capability | native UI | web content |
|---|---|---|
| inspector picking | works | **works** (hit test) |
| inspector hover preview | instant | lags to the hit test, ~110 ms |
| `get_tree` / `--llm` output | works | **blind** |
| CSS-like selectors, `--click`, `--query` | works | **blind** |
| tapping by coordinate | works | works |

A hybrid app is therefore drivable by an agent that can see the screen, and
invisible to one that reasons over the tree. For a Capacitor app that is
essentially the whole UI.

## What idb does, and what that settles

Read from a clone of idb at `~/Dev/idb`.

### SimulatorBridge is dead — they deleted it

`accessibilityElementsWithDisplayId:` exists **only** in
`PrivateHeaders/SimulatorBridge/SimulatorBridge-Protocol.h:19`. Nothing in idb
calls it. `git log -S accessibilityElementsWithDisplayId` tells the whole
story:

    d821fc904 Add SimulatorBridge Headers
    a12a016b2 Add Accessiblity Elements API for Simulators
    276435588 Delete FBSimulatorBridge

The only surviving trace is the output *format*, kept for compatibility —
`FBSimulatorAccessibilitySerializer.swift:78` notes the values "mirror the old
SimulatorBridge implementation for downstream compatibility". `SimulatorBridge`
is otherwise referenced only to restart `com.apple.CoreSimulator.bridge` as
SpringBoard-crash remediation, which we already do.

**So option B is dead.** Had we built it, we would have reimplemented
something its own authors removed.

### idb's current mechanism is the same as ours

`FBSimulatorAccessibilityCommands.swift:114`:

```swift
// Uses the CoreSimulator accessibility API via
// -[SimDevice sendAccessibilityRequestAsync:completionQueue:completionHandler:].
```

`AXPTranslator` with a `bridgeTokenDelegate`, and exactly two request kinds —
`.frontmostApplication` and `.point(point)`. That is our implementation. It
would inherit the identical blindness.

### And they solve web content by grid hit testing

`FBControlCore/Commands/FBAccessibilityRequestOptions.swift:11`:

```swift
/// Options for fetching remote process elements (e.g., WebView content).
/// Remote elements are in separate processes and require grid-based hit-testing.
```

That is first-party confirmation that no better API exists. The implementation
is `discoverRemoteElements` in `FBSimulatorControl/Commands/FBAXTranslationRequest.swift:160-247`,
and it is more careful than a naive sweep in four ways worth copying:

1. **A coverage grid, populated during the ordinary recursive traversal**
   (`FBAccessibilityCoverageGrid.swift`, cell-based `markFilled` / `isFilled`).
   Probe points that already sit on a natively-discovered element are skipped
   entirely, so the sweep only pays for the parts of the screen the tree walk
   could not explain.
2. **Remote content is identified by pid.** Hits whose pid was already seen in
   the traversal, or which belong to the frontmost app, are discarded. A
   different pid *is* the signal that something came from another process.
3. **Dedup by frame**, since a 50pt grid lands on a large element repeatedly.
4. **Provenance is recorded.** Every element carries a discovery method of
   `"recursive"` or `"point_grid"` plus an `isRemote` flag, and the response
   reports frame coverage before and after. Consumers can tell exact results
   from sampled ones.

Defaults: `gridStepSize = 50.0` points, full-screen region, `maxPoints = 0`
(unlimited).

### Why the experiment would have misled us

`remoteContentOptions` defaults to `nil`, meaning **remote content is not
fetched unless explicitly requested**, and the option is not plumbed through
the proto or the Python CLI at all — `grep` for `remote_content` across
`proto/` and `idb/` returns nothing.

So `idb ui describe-all` on a web page would have shown no web content, and we
would have concluded idb could not see it either. Reading the source was the
right call.

## Plan

Option A, following idb's design.

1. **Build the coverage grid during the existing tree walk.** Cheap, useful on
   its own: the coverage ratio is a direct measure of how much of the screen
   the tree explains, which is the diagnostic that would have made this whole
   problem obvious months earlier.
2. **Add a sweep over uncovered regions**, skipping filled cells, filtering by
   pid, deduping by frame. Start at a 50pt step to match idb. At the measured
   3.2 ms per probe, a full-screen 50pt grid on a 402x874 point device is
   ~8x17 = 136 points worst case, or ~0.44 s, and far less once covered cells
   are skipped.
3. **Tag provenance** — `recursive` vs `point_grid`, and a coverage figure on
   the response. Do not let sampled results masquerade as exact ones.
4. **Expose it as opt-in**, not as part of every `get_tree`. A `--scan` flag on
   the CLI and a query parameter on `/api/ax/tree`.
5. **Independently, fix the inspector's hover preview** by caching hit-test
   results as the pointer moves. Picking already works on web content; only the
   instant preview is missing. Smallest useful change here and unblocked by
   everything above.

## What not to do

Do not make `get_tree` sweep transparently. It would turn a fast call into a
half-second one and silently return a flat, approximate list under the same
shape as the exact hierarchy returned for native apps.

Do not implement SimulatorBridge. See above.

## Remaining unknown

Whether a 50pt grid is right for phone-sized screens, and whether adaptive
refinement (denser probing where returned frames are small) is worth it. idb
ships a fixed step, which is evidence that fixed is good enough.
