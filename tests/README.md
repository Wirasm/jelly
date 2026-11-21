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
