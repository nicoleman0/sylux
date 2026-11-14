# PDF reader: SYL-51 and SYL-52

Research and implementation decision, 14 November 2026.

## Reference workflows

- [Nextcloud's viewer shell](https://github.com/nextcloud-libraries/nextcloud-viewer/blob/fa485addce30ec97af5d773013527faf3590da34/lib/views/Viewer.vue) uses a full-window modal, compact header actions and a loading/error state around a file handler. Closing returns to the file workspace. It is not a separate browser popup.
- [Nextcloud's PDF handler](https://github.com/nextcloud/files_pdfviewer/blob/3f1f56ec1226562a105b2ce2c8e88a610eccc330/src/views/PDFView.vue) embeds the complete PDF.js application in a same-origin iframe and bridges its initialization, permissions and download controls. This gives it the upstream toolbar, text layer, search and scrolling rather than a single canvas.
- [Dropbox preview](https://help.dropbox.com/view-edit/preview) offers full-screen and quick-view modes. Its [document search](https://help.dropbox.com/view-edit/search-text-in-file) uses full-screen preview, keyboard Find, highlighted matches and next/previous navigation.
- Visually inspected the [live Mozilla reader](https://mozilla.github.io/pdf.js/web/viewer.html): a thin toolbar with page/zoom controls leaves most of the viewport for white document pages on a neutral background; consecutive pages share one scroll area. Nextcloud and Dropbox were source/documentation comparisons, not authenticated live-instance tests.

## Decision

Use the maintained `PDFViewer`, `PDFFindController` and `PDFLinkService` components from pinned `pdfjs-dist` 6.5.438 inside Sylux's full-window dialog. Keep the existing authenticated, bounded byte fetch and local worker/font/CMap/WASM assets. Use Sylux's controls for page, zoom, find, close and download.

The complete iframe application was considered. It is a useful model, but `pdfjs-dist` ships the viewer components rather than the full viewer HTML/application/localization tree. Vendoring that application adds another asset/build surface and duplicates the host controls. The component integration keeps the rendering queue, text layer, search and detail canvases upstream while leaving a small application toolbar to maintain. Its pinned API and CSS require regression checks on upgrades.

Initial automatic scaling avoids stretching small pages to the full desktop width. Page width, page fit and explicit percentages remain available. Search is in-document text search, not OCR. Scanned pages remain images.

No Nextcloud source is copied. PDF.js remains under its existing Apache-2.0 dependency notice. Saved annotation appearances render, but their DOM layer is inert. PDF scripting, annotation editing, interactive forms and automatic external links are not enabled.

## Blank-page diagnosis

The old renderer passed `maxImageSize: 16_000_000`. With a synthetic 4100×4100 scan, PDF.js returned an empty operator list at that setting despite `stopAtErrors: true`; removing the cutoff returned the image operator. The owner's supplied 44-page PDF reproduced the same result on pages 1, 2, 22 and 44: zero operations with the old setting and an image on every sampled page without it. The private document is not checked in.

The old renderer also scaled a capped raster to the full container width and labelled fit-width-relative scale as “100%”. This made high-DPI and enlarged content look degraded.

The replacement gives base page canvases a 4M-pixel / 4096-side budget and uses PDF.js's viewport detail canvas for zoom. The upstream detail canvas covers at least the visible viewport at native device-pixel ratio, so its bound is the larger of 4M pixels and that viewport area, allowing edge rounding. It does not inherit the base canvas's side cap. A 1280×900, 2× DPR Chrome test measured a 4,126,788-pixel detail canvas. Larger or higher-DPI screens can allocate more. Embedded images are downsampled through the decoder canvas budget rather than discarded by an image-pixel cutoff. The existing 20 MiB input and 500-page limits remain. These limits constrain particular resources, not total browser memory; image decoding, text extraction and worker allocations also consume memory.

## Verification

Committed fixtures are synthetic and generated locally: searchable mixed-size pages, a 16.81-million-pixel scan with a dark center target, a genuine blank page, a saved annotation and a long document. Browser tests exercise the actual PDF.js worker, text search/selection, scrolling, scaling, bounded canvases, cleanup, retry and both themes at desktop/mobile widths. Mocked viewer tests separately exercise stalled initialization/render deadlines and stale completion events.

The real TLS storage journey uses the existing three-page fixture and checks page navigation and focus restoration. Running web tests with mocked HTTP is not evidence that the TLS journey or container recovery ran. See the PR for the exact checks executed and remaining limits.

Local final checks: Svelte check and production build passed; 45 Chrome PDF checks and 27 production-build Firefox checks passed. The corrected viewport-based high-DPI assertion passed three further Chrome runs. The supplied document passed local pixel-content checks on pages 1, 2, 22 and 44 at 1280px and 390px widths. Desktop/mobile screenshots were reviewed. Safari, physical mobile devices, screen readers, real TLS storage and container recovery were not exercised in this batch.
