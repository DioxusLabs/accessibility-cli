---
name: ios-simulator-accessibility
description: Reading the iOS Simulator accessibility tree and hit testing it — bridge delegate tokens, full-screen backdrops, app-scoped versus display-scoped results, and reaching web content that the tree cannot see. Use when working on get_tree, element picking or the inspector.
triggers:
  - user
  - model
---

Reading and hit testing the simulator's accessibility tree, via `AXPTranslator`
in `packages/accessibility-ios-sys`.

The theme: failures here are **silent and look like absence**. A missed token
returns an empty label rather than an error, so the symptom is "this element
cannot be selected" rather than anything that points at the cause.

## Hit testing needs the token applied twice

`objectAtPoint:` returns a platform element whose *own* translation must also
be given the bridge delegate token — it is not necessarily the translation you
tokenized on the way in.

Miss that and attribute reads do not fail, they silently return an empty label
and a zero frame. `get_tree` already does this; see the matching step in
`get_element_at_point`.

## SpringBoard crash remediation

A dead SpringBoard can leave CoreSimulatorBridge serving a stale root with a
zero frame. Spawn the runtime's `bin/launchctl` through
`SimDevice.spawnAsyncWithPath:...` and run `stop com.apple.CoreSimulator.bridge`.
The service is kept alive and respawns automatically; after it exits, retry the
accessibility query once. This was verified on Xcode 26.6 by killing SpringBoard:
the retry returned a healthy tree from the replacement SpringBoard PID.

The spawn completion and termination callbacks share one `Condvar`, so their
results must also share one mutex. macOS rejects using one condition variable
with two mutexes and panics before the CoreSimulator call can complete.

## Backdrops swallow hit tests

Every app has full-screen backdrops: the Application node plus one or more
container groups. Hit testing empty space resolves to one, and highlighting it
paints over the whole device. They are filtered out rather than drawn.

## The tree and the hit test have different scopes

The tree is app-scoped and the hit test is display-scoped, so the status bar
appears in hit tests but never in `get_tree`.

## Reaching web content

`get_tree` walks the frontmost app and so cannot see anything in another
process; a Safari page reports five elements of chrome. Hit testing *does*
cross that boundary — `objectAtPoint:` resolves elements inside a `WKWebView`
while `get_tree` returns only the host app's chrome, and there is no hierarchy
to traverse from either end. This applies to Safari and to any embedded web
view.

So `?scan=true` marks everything the tree walk explained on a coverage grid and
then probes the cells left over. On a real page that takes 5 elements at 4%
coverage to 15 at 50%, using 114 probes in under half a second.

Swept elements are tagged `point_grid` rather than `recursive`: they are point
samples with no parent, no children and no document order, and should not be
treated as equivalent to nodes the tree walk returned.

Coverage is reported whether or not a scan runs, and is a useful signal on its
own — a full web page reporting 4% describes the problem far better than an
empty element list does.

Background on the approach, and how idb solves the same problem, is in
`docs/WEB_CONTENT_ACCESSIBILITY_PLAN.md`.
