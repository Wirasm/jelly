# Full-view header browser check

Requires Cargo, Chrome and `playwright-cli`. Build from the checkout and start a
fresh throwaway daemon (substitute your scratch paths and port):

```sh
cargo build --locked --target-dir /tmp/jelly-header-target
/tmp/jelly-header-target/debug/jelly serve --port 17849 --home /tmp/jelly-header-home
```

From a scratch directory, run the check with an absolute path to the script:

```sh
playwright-cli -s=header open http://127.0.0.1:17849/
playwright-cli -s=header run-code --filename=/path/to/jelly/tests/full-view-header.js
playwright-cli -s=header close
```

The script creates and cleans up its own fixtures. It uses a mobile browser context
at 390px and checks that the layout viewport matches the device width at full scale.
It checks title and metadata geometry, action reachability, wrapping and truncation
at 320, 390, 900, 901 and 1280px. At phone and desktop widths it also checks
Comment, iframe Escape, the parent c shortcut, Undo, tab order, Pin/Unpin, Back,
Enter, Escape and Remove.
It returns measurements and saves `header-390.png` and `header-1280.png` in the
scratch directory. Stop the throwaway daemon afterward. Existing homes retain
their stored jelly page; use a fresh home for the newly built bundled view.

# Shared-file browser check

`cargo test` includes the shared-file browser regression. Chrome/Chromium is
required; set `JELLY_CHROME` to its executable if it is not installed in a usual
location. A missing browser fails the check rather than skipping it.

To run only the sharing checks:

```sh
cargo test --locked --bin jelly server::share_tests -- --nocapture
```

The test exports a jelly to a temporary HTML file, opens it through `file://`
and checks image pixels, CSS resources and keyframe edits, native CSSOM calls,
open/closed imperative and declarative shadow roots, unchanged inert templates
and custom-element registrations, dynamic objects using `jelly.asset` (implicit
dynamic object paths remain unchanged), static objects in open/closed declarative
roots (including rendered image dimensions), inert object markup, uploaded stylesheets
(including lowercase and mixed-case stylesheet relationship tokens, late link charset
selection, `jelly.asset` resolution and imports) and scripts, collection of
discarded roots with continued observation of live closed roots, captured
attention readers and read-only mutations.
It uses a temporary browser profile and a loopback test-result receiver, and
requires no daemon or real jelly home.

Separate Rust tests in `server::share_tests` assert against the exported HTML to
check HTML base eligibility and location-independent static object asset
resolution. These checks do not open the file in a browser and run with the same
command above.

# Stream header browser check

Requires Cargo, Chrome and `playwright-cli`. Build from the checkout and start a
fresh throwaway daemon (substitute your scratch paths and port):

```sh
cargo build --locked --target-dir /tmp/jelly-stream-target
/tmp/jelly-stream-target/debug/jelly serve --port 17850 --home /tmp/jelly-stream-home
```

From a scratch directory, run with an absolute path to the script:

```sh
playwright-cli -s=stream open http://127.0.0.1:17850/
playwright-cli -s=stream run-code --filename=/path/to/jelly/tests/stream-header.js
playwright-cli -s=stream close
```

The script creates and deletes only its own fixtures through the existing API.
It checks the actual served stream in a touch-enabled mobile context at 390px,
with device-width and scale assertions, and at 320, 900, 901 and 1280px. Normal,
long and unbroken titles and metadata, status badges, activity, and expanded
Comment labels must fit without overlap or horizontal overflow. It checks action
hit targets, responsive visibility, desktop reveal, forward/reverse tab order,
visible keyboard focus, Comment and iframe Escape, c, j/k, Enter, Open, Back,
Pin persistence, and cancelled/confirmed Remove. Project and All identification
are checked at 390 and 1280px.

It returns measurements and saves `stream-390.png`, `stream-390-expanded.png` and
`stream-1280.png` in the scratch directory. Inspect these images for readability.
Run `full-view-header.js` against the same daemon to check full-view behavior.
Stop the daemon afterward with Ctrl-C. Existing homes retain their stored jelly
page; use a fresh home to test the newly built bundled view. To explicitly update
a stored page, use `/tmp/jelly-stream-target/debug/jelly --url http://127.0.0.1:17850 shape page --view assets/page.html`
from the checkout, merging these changes into any customized view first.

# Comment Escape browser check

Requires Cargo, Chrome and `playwright-cli`. Build from the checkout, start a fresh
throwaway daemon and make the map recipe jelly the check expects (substitute your
scratch paths and port):

```sh
cargo build --locked --target-dir /tmp/jelly-escape-target
/tmp/jelly-escape-target/debug/jelly serve --port 17893 --home /tmp/jelly-escape-home
# In another terminal, from the checkout:
/tmp/jelly-escape-target/debug/jelly --url http://127.0.0.1:17893 new escape:map --from skill/recipes/map
```

From a scratch directory, run with an absolute path to the script:

```sh
playwright-cli -s=escape open http://127.0.0.1:17893/
playwright-cli -s=escape run-code --filename=/path/to/jelly/tests/comment-escape.js
playwright-cli -s=escape close
```

For the map standalone and full size on the jelly page, it walks a trace to step 3,
then checks that Escape ending comment mode and Escape cancelling the comment
editor keep that step, and that the next Escape closes the trace (and, embedded,
the full view). The Escape-ends-commenting check fails on the runtime before the fix;
the editor check passes either way (the map ignores keys in the editor) and guards
that editor cancellation keeps working.
Stop the daemon afterward.

# Shape-in-place browser check

Requires Cargo, Chrome and `playwright-cli`. Build from the checkout and start a
fresh throwaway daemon (substitute your scratch paths and port):

```sh
cargo build --locked --target-dir /tmp/jelly-shape-target
/tmp/jelly-shape-target/debug/jelly serve --port 17894 --home /tmp/jelly-shape-home
```

From a scratch directory, run with an absolute path to the script:

```sh
playwright-cli -s=shape open http://127.0.0.1:17894/
playwright-cli -s=shape resize 1280 900
playwright-cli -s=shape run-code --filename=/path/to/jelly/tests/shape-in-place.js
playwright-cli -s=shape close
```

The script makes and removes its own jelly. Standalone, on the stream and full
size, it scrolls the page and a box, types in a field, shapes, and checks that
the new version kept the scroll, the box's scroll, focus, the typed text and its
selection. It also shapes a pinned tile. On the jelly page a per-frame sampler
fails the check if a visible frame reloads in place or shows an unloaded
document, and verb calls during a swap must not stall it. Run against a jelly page
from before the swap (`git show <old>:assets/page.html`, then `jelly shape
page`), the sampler and focus checks fail. Existing homes keep their stored
jelly page; use a fresh home or shape `page` with `assets/page.html`.
Stop the daemon afterward.
