# Attention visibility browser check

Requires Cargo, Chrome and `playwright-cli`. Use a throwaway daemon home. From the
checkout, build and install the fixture (substitute your own scratch path/port):

```sh
cargo build --locked --target-dir /tmp/jelly-visibility-target
printf 'const verbs = {};\n' > /tmp/jelly-visibility-verbs.js
/tmp/jelly-visibility-target/debug/jelly serve --port 17881 --home /tmp/jelly-visibility-home
# In another terminal, from the checkout:
/tmp/jelly-visibility-target/debug/jelly --url http://127.0.0.1:17881 new visibility:fixture \
  --view tests/attention-visibility.html --verbs /tmp/jelly-visibility-verbs.js
```

Run the browser from a scratch directory so generated browser files stay outside
the checkout. A home created by an earlier build retains its stored jelly page;
use a fresh home, or update that test page with
`jelly --url http://127.0.0.1:17881 reshape surface --view assets/surface.html`
from the checkout before running. The program takes the daemon URL from the open page and creates
fresh jellies through the API. Use an absolute path to the test:

```sh
playwright-cli -s=visibility open http://127.0.0.1:17881/p/visibility
playwright-cli -s=visibility resize 1280 900
playwright-cli -s=visibility run-code --filename=/path/to/jelly/tests/attention-visibility.js
playwright-cli -s=visibility resize 390 844
playwright-cli -s=visibility run-code --filename=/path/to/jelly/tests/attention-visibility.js
playwright-cli -s=visibility close
```

The check clicks real dock rows and asserts visibility, shadow-root focus,
context, measured diagnostics, host/draft/selection preservation, synchronous and
delayed reveals, subscription behavior, stable shared destinations, persisted
answers, grouped inline questions with empty and punctuation IDs, readable grouped
question headings alongside visible focus below the sticky bar, tall preview and
long-context openings of later grouped questions with focus inside the requested
question and without unseen Enter submissions, hash/query/path
history navigation, attribute-driven opacity, visible refresh and focus-write
failures with retry, delayed focus writes and agent
focus during local open/close writes, and snapshot registration.
Hidden-section visibility fails on the pre-fix runtime. Stop the throwaway daemon
after testing; no real jelly home is used.
