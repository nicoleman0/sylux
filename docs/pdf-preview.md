# PDF preview compatibility

Sylux uses PDF.js 6.5.438 to render PDF previews in the browser. This version calls `Map.prototype.getOrInsertComputed()` and `WeakMap.prototype.getOrInsertComputed()`. Sylux checks for both methods before loading the renderer. If either is missing, the preview explains that the browser needs an update and keeps the download action available.

The earliest listed browser releases with both methods are:

| Browser | Minimum version |
| --- | ---: |
| Chrome and Edge | 145 |
| Firefox | 144 |
| Safari and Safari on iOS | 26.2 |

These versions follow Mozilla's browser compatibility data for [Map](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Map/getOrInsertComputed) and [WeakMap](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/WeakMap/getOrInsertComputed). They describe these two methods only, not a complete PDF.js browser support guarantee. The runtime feature check is authoritative for this requirement: Chromium derivatives, embedded browsers and modified runtimes can differ from these release numbers. PDF.js also documents [its browser support policy](https://github.com/mozilla/pdf.js/wiki/Frequently-Asked-Questions#which-browsersenvironments-are-supported).

The check uses browser capabilities, not user-agent strings. Sylux does not add polyfills or use PDF.js's legacy build for this requirement.

## Verification

Run `npm --prefix web exec -- playwright install chromium firefox`, then from `web` run `npx playwright test --config playwright.pdf.config.ts`. This focused suite includes Firefox with classic scrollbars forced at launch and checks that the scrollbar consumes layout width before exercising the real PDF fixture. On macOS, Firefox runs headed because its headless mode does not allocate native scrollbar width. It opens a test window in a disposable profile and does not change macOS settings.

The launch preference follows [Firefox's macOS testing guidance](https://firefox-source-docs.mozilla.org/ai-agent-tools/skills/desktop-theme-probing/references/platform-macos.html). Automated capability tests remove each required method independently and verify that the renderer is not loaded. Lifecycle tests cover zero width, stalled page/render work, repeated resizes, timeout recovery and cleanup.
