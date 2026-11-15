# UI controls and sorting research

Inspected 2026-11-15 before implementation. Sylux baseline: `origin/main` at `10fc3e3e0bd4c4711ddfdea4e17f72c90ba45ca5`.

## Evidence

- **Nextcloud 35:** [Files web interface](https://docs.nextcloud.com/server/stable/user_manual/en/files/access_webgui.html) and [screenshot](https://docs.nextcloud.com/server/stable/user_manual/_images/users-files.png). The screenshot shows a compact, flat list with Name, Size and Modified columns and a direction arrow on the sorted heading. Its upload control is a compact menu. It does not show routine empty-folder guidance.
- **File Browser:** [`FileListing.vue` on master](https://github.com/filebrowser/filebrowser/blob/master/frontend/src/views/files/FileListing.vue) has titled, aria-labelled Name, Size and Modified sort controls. [Screenshot](https://raw.githubusercontent.com/filebrowser/filebrowser/master/docs/static/2.jpg) shows a direction arrow on Name and a compact icon toolbar. The screenshot identifies itself as `v(untracked)`: it is a historical, unversioned fixture, not evidence of a live test.

## Decisions

- Adopt compact named controls and sortable headings with a visible direction indicator.
- Keep Sylux's existing colors and icon treatment; do not copy competitor styling or icon fonts.
- Keep file sorting semantics: Name remains the default. In date and size modes, unknown values sort last in either direction, folders remain first, and metric ties use name, then Location for search, then ID. Location sorts use ancestor paths with name/ID ties.
- Account tables sort their complete local list by Name, Role or Status.

## Limits

These are source and screenshot inspections, not live competitor tests. The File Browser screenshot is unversioned and may not match current behavior.

## Verification

- Pinned Node 24.22.2 and npm 11.19.1: Svelte check (no errors or warnings), production build, and the full frontend suite: **399 passed, 1 failed**.
- The failure is `PDF a paused render retains a bounded wait after resuming without another render event` in `web/tests/pdf-render.spec.ts`. The unchanged main revision above also fails it when running the complete PDF render spec (17 passed, 1 failed). Running that single test passes on both main and this branch. This establishes a pre-existing test-order failure; it does not establish a production PDF timeout defect. PDF runtime code is unchanged.
- All 15 new column-sorting journeys pass, including held-request focus and duplicate prevention. Stable renders inspected in both themes at 375px, 900px and 1280px; the 900px long-name/path journey passes. Header cells remain accessible when their visual columns collapse.
- Rust: 48 names/listing integration tests, 2 search tests including the existing 10k/100k query-plan checks, fmt and Clippy passed. Eight screenshot-path and workflow-measurement helper checks passed. The Impeccable detector reported no findings.
- New sorting and Trash validation specs run in normal PR CI. Real TLS browser and container recovery checks did not run locally. No release or deployment was performed. No new latency or capacity benchmark is claimed; Trash Location sorting constructs ancestor keys in SQL before bounding the returned page.
