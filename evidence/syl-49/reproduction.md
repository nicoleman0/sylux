# SYL-49 PDF resize assertion

## Cause and fix

The test sampled the render count during the dialog's native CSS transition.
It did not establish that the viewport resize had finished. The dialog has
`transition-property: all` and a 100 ms duration from its `duration-100` class
in [dialog-content.svelte](../../web/src/lib/components/ui/dialog/dialog-content.svelte).
Chrome reported running transitions for width, max-width, height and max-height
after the viewport changed from 1280 × 900 to 900 × 800.

An instrumented failing run recorded this sequence:

| Event | Mocked time | Container client width | Render count |
| --- | ---: | ---: | ---: |
| Resize and render | 2117 ms | 781 px | 6 |
| Test sampled count | 2133 ms | 781 px | 6 |
| Final resize and render | 3133 ms | 775 px | 7 |

The native transition kept advancing while the JavaScript clock was paused.
Each changed width caused one render. The final render was needed for the
final width; this trace does not show repeated rendering at an unchanged width.

[pdf-render.spec.ts](../../web/tests/pdf-render.spec.ts) now waits for the
dialog's actual animations to finish, then polls for a canvas fitted to the
final container width and confirms `aria-busy=false`. It still requires a new
render after resize, an exactly unchanged render count over the next 1000 ms
of mocked time, and one page load. The poll has Playwright's 5-second bound;
the test has a 30-second bound in the recorded configuration. No retries or
render-count tolerance were added. PdfPreview.svelte was unchanged.

## Environment and results

- Date: 2026-11-13, macOS arm64.
- Clean baseline: `f7461e012052389780029d1a568bc3fc5ad210a2`.
  The baseline finished before concurrent SYL-44 App.svelte edits began.
- Node 24.22.2, npm 11.19.1, Playwright 1.65.0.
- Google Chrome 157.0.8103.51, headless, one worker, retries 0.
- Unchanged test: 19/20 passed; one failure expected 6 renders and received 7.
- Instrumented unchanged test: 29/30 passed; the failure produced the trace above.
- Corrected test: 30/30 passed in 39.5 seconds.
- Full Chrome PDF selection: 18/18 passed in 13.4 seconds. This includes the
  real three-page fixture at 1280 px and 390 px, page/zoom controls, malformed
  input recovery, rapid page changes and close/reopen cleanup. Those tests were
  unchanged.
- Firefox 159.0, headed with `ui.useOverlayScrollbars=0`: the same 18/18 PDF
  checks passed in 21.7 seconds. The real-fixture tests asserted that native
  scrollbars consumed layout width. The first temporary Firefox config kept
  Chrome's channel through config inheritance and could not launch; removing
  that inherited option allowed the recorded run.
- `npm --prefix web run check`: zero errors and warnings.

## Commands and raw output

Commands ran from the repository root with
`PATH=/private/tmp/sylux-node24/bin:$PATH`. The Chrome configuration at
`/private/tmp/sylux-batch-playwright.config.mjs` used the repository's
`web/tests`, `browserName: 'chromium'`, `channel: 'chrome'`,
`headless: true`, viewport 1280 × 900, timeout 30000, one worker and retries 0.
It started the pinned Node runtime's Vite server on 127.0.0.1:4178 and reused
an existing server when present.

```sh
node web/node_modules/@playwright/test/cli.js test \
  --config=/private/tmp/sylux-batch-playwright.config.mjs \
  web/tests/pdf-render.spec.ts --grep 'canvas layout' --repeat-each 20 \
  --output=/private/tmp/syl-49-baseline-results --reporter=list,json \
  > /private/tmp/syl-49-baseline.log 2>&1

node web/node_modules/@playwright/test/cli.js test \
  --config=/private/tmp/sylux-batch-playwright.config.mjs \
  web/tests/pdf-render.spec.ts --grep 'canvas layout' --repeat-each 30 \
  --output=/private/tmp/syl-49-fixed-repeat2-results --reporter=list,json \
  > /private/tmp/syl-49-fixed-repeat2.log 2>&1

node web/node_modules/@playwright/test/cli.js test \
  --config=/private/tmp/sylux-batch-playwright.config.mjs \
  web/tests/pdf-render.spec.ts web/tests/pdf-compatibility.spec.ts \
  web/tests/preview.spec.ts --grep PDF \
  --output=/private/tmp/syl-49-full-chrome-results --reporter=list,json \
  > /private/tmp/syl-49-full-chrome.log 2>&1

PLAYWRIGHT_BROWSERS_PATH=/private/tmp/sylux-pdf-firefox \
node web/node_modules/@playwright/test/cli.js test \
  --config=/private/tmp/syl-49-firefox-classic.config.mjs \
  web/tests/pdf-render.spec.ts web/tests/pdf-compatibility.spec.ts \
  web/tests/preview.spec.ts --grep PDF \
  --output=/private/tmp/syl-49-full-firefox2-results --reporter=list,json \
  > /private/tmp/syl-49-full-firefox2.log 2>&1
```

The Firefox config imports the Chrome config, removes `use.channel`, sets
`browserName: 'firefox'`, `headless: false` and
`launchOptions.firefoxUserPrefs['ui.useOverlayScrollbars']: 0`, and names its
single project `firefox-classic`.

Diagnostic source, config and 30-run observer trace remain in
`/private/tmp/syl-49-probe/`, `/private/tmp/syl-49-probe.config.mjs`,
`/private/tmp/syl-49-probe.log` and `/private/tmp/syl-49-probe-results/`.
The separate computed-transition probe is in
`/private/tmp/syl-49-transition.log`. These temporary files are local evidence,
not committed artifacts.
