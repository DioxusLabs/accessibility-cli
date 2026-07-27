# Reaching web content in the accessibility tree

Plan for exposing `WKWebView` and Safari content, which the ordinary tree walk
cannot see.

## What was measured

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
including the round trip. That is the number that makes option A viable.

## How this affects WebView apps

Identically, and this matters more than the Safari case.

Safari's content area *is* a `WKWebView` backed by a separate WebContent
process. An app embedding `WKWebView` gets exactly the same architecture —
there is no in-process mode. So the measurements above apply unchanged to:

- React Native `WebView`
- Capacitor and Cordova apps, which are almost entirely web content
- in-app browsers and `SFSafariViewController`
- any native app with an embedded `WKWebView`

The practical consequences, in order of how much they hurt:

| capability | native UI | web content |
|---|---|---|
| inspector picking | works | **works** (hit test) |
| inspector hover preview | instant | lags to the hit test, ~110 ms |
| `get_tree` / `--llm` output | works | **blind** |
| CSS-like selectors, `--click`, `--query` | works | **blind** |
| tapping by coordinate | works | works |

So a hybrid app is drivable by an agent that can see the screen, and invisible
to one that reasons over the tree. For a Capacitor app that means essentially
the whole UI is missing from every tree-based tool, while the element picker in
the browser works fine.

## Options

### A. Bounds-guided hit-test sweep

Enumerate by probing. Start at the top-left of the web area, hit test, record
the element and its bounds, then step past its right edge and repeat; at the
end of a row, step down past the shortest element in it. Because every probe
returns the element's full bounds, the walk is roughly O(number of elements)
rather than O(area).

- **Cost**: a page with ~40 elements needs perhaps 60-150 probes, so 0.2-0.5 s
  at the measured 3.2 ms. Fine on demand, fine as a background refresh, too
  slow per-hover.
- **Pros**: uses a primitive already proven to work, no new dependencies, and
  it fixes *any* out-of-process content, not just WebKit.
- **Cons**: it is sampling. Elements smaller than the step are missed,
  fully-occluded elements are unreachable by construction, and the result is a
  flat list with no parent/child structure or document order — so selectors
  like "the third row in this section" remain impossible.
- **Mitigation**: refine adaptively, probing denser inside regions where the
  returned bounds are small.

### B. SimulatorBridge over Distributed Objects

What idb uses. A bridge process runs *inside* the simulator and
`accessibilityElementsWithDisplayId:` returns every element as JSON in one
call, from inside the simulated OS rather than across the host boundary.

- **Unverified, and this is the crux.** It is not established that
  SimulatorBridge sees web content either; it may sit on the same
  AXPTranslator plumbing and inherit the same blindness. Nothing should be
  built here until that is answered.
- **Pros, if it works**: one call, full hierarchy, correct ordering — strictly
  better than sampling.
- **Cons**: a much larger lift. Distributed Objects connection, bridge
  lifecycle and versioning, and a dependency on a private service whose shape
  changes between Xcode releases.

### C. Talk to WebKit's accessibility directly

VoiceOver reads web content on a real device, so the information is reachable
in principle; WebKit vends it through remote element tokens. This is the
deepest option and the least charted. Not worth considering unless both A and
B fail.

## Recommended order

1. **Answer B with a 30-minute experiment, before writing any code.**
   `brew install idb-companion`, put the probe page on screen, run
   `idb ui describe-all`, and look for `Probe Link Alpha`. If it appears,
   SimulatorBridge sees web content and option B is worth the work. If it does
   not, B is dead and the choice is made for us.
2. **Ship the cheap inspector win regardless.** Picking already works on web
   content; only the instant hover preview is missing because it reads the
   cached tree. Caching hit-test results as the pointer moves builds up a local
   map for free and makes web areas feel the same as native ones. Small, self
   contained, no dependency on the outcome of step 1.
3. **Then implement A or B** depending on step 1, exposing it as an explicit
   `scan` rather than something that happens on every tree fetch — a
   half-second sweep should be opt-in.
4. **Mark web content in the output either way.** Once elements arrive from a
   different mechanism than the tree walk, consumers should be able to tell
   which is which, if only to explain why ordering is missing.

## What not to do

Do not make `get_tree` transparently sweep. It would turn a fast call into a
half-second one, and it would silently return a flat, approximate list under
the same shape as the exact hierarchical one it returns for native apps.
