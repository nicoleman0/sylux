# Dev-only typography experiment

The experiment compares four reversible variants using the self-hosted JetBrains Mono 400 Latin font already in the web dependencies:

- Baseline: current styles.
- Metadata: file sizes, dates, account roles, and secondary file details use monospace.
- Headings: metadata and sortable table headings use monospace.
- Filenames: metadata, headings, and filenames use monospace.

In development, add `?typography=metadata`, `?typography=headings`, or `?typography=filenames` to the current URL. Remove the parameter for baseline. The catalogue links switch variants while keeping the current route, other query parameters, and hash. Its application links open the same variant in the app.

The experiment CSS applies only under the `data-typography-experiment` root attribute. `initTypographyExperiment()` sets that attribute only when `import.meta.env.DEV` is true. Initialize it before mounting both the app and catalogue. The font is loaded only with an active dev variant. Production keeps the current type styles and shared tokens.

Catalogue rows include a long filename, a large size, and a date for desktop and narrow-screen review in both themes. The chosen direction remains open until the owner reviews the rendered samples.

## Baseline source assessment

- Authority and fit: `web/src/tokens.css` sets `--font-ui` to the platform system sans stack and `--font-code` to `ui-monospace, SFMono-Regular, Consolas, monospace`. That suits an operational file workspace. The baseline stays on these stacks; variants load JetBrains Mono 400 to keep the comparison consistent across platforms.
- Hierarchy and scale: the role sizes are 11px note, 12px metadata, 13px interface, 16px brand, 18px section, and 23–25px page headings. `web/src/app.css` applies these roles to `body`, `.list-head`, `.file-size`, and `h1`; metadata and interface differ by only 1px, so the test checks whether a fixed-width face improves scanning without making details too prominent.
- Reading and density: file rows are dense, with 44px minimum height and 13px interface text (`.file-row`, `.file-size` in `web/src/app.css`). Supporting prose is limited to 65ch in `.section-description`. A body-reading surface is not the target of this experiment.
- Stress: `.filename` ellipsizes in wide rows, expands on row hover or focus, and wraps at widths up to 620px. The catalogue sample adds a long name, a 1.15 GB size, and a localized date for layout review. Catalogue media rules switch to two control columns below 680px.
- Delivery: baseline UI uses system faces. An active dev variant loads only JetBrains Mono Latin 400 with `font-display: swap`; it adds no production font request. `font-synthesis: none` is set in `web/src/app.css`.

The mechanical typography scan reported no findings for the baseline App, CSS, tokens, and catalogue source. It cannot determine whether monospace improves the rendered hierarchy; that remains an owner comparison.
