# Sylux design system

Sylux is a quiet file workspace. Keep its own system sans-serif type, green-gray surfaces and teal actions. Files and account tasks take priority over decoration.

## Source of truth

- `web/src/tokens.css` owns semantic colors, type sizes, control dimensions, corners and overlay shadow.
- `web/src/app.css` connects those tokens to Tailwind and the application layout.
- `web/src/lib/components/ui/` owns shared control behavior and variants.
- `/design-system.html` is the local catalogue. It renders the actual CSS and components and reads token values from the browser. Run `npm --prefix web run dev` to view it. It is not included in the production build.

Change a shared value in the token file, rather than overriding each consumer. This document explains intent; the catalogue shows current values.

## Typography

Use `--font-ui` for the interface and `--font-code` for file content or code that needs fixed-width characters. Use `--weight-heading` for semibold headings, `--weight-body` for ordinary content and `--weight-control` for control labels.

| Role | CSS token | Use |
| --- | --- | --- |
| Page | `--type-page` | Main destination heading |
| Compact page | `--type-page-compact` | Page heading at narrow widths |
| Section | `--type-section` | Settings sections and result headings |
| Brand | `--type-brand` | Instance name in the sidebar |
| Interface | `--type-ui` | File names, body text, controls and navigation |
| Metadata | `--type-meta` | Column labels, secondary file details and account roles |
| Note | `--type-note` | The persistent beta notice |

Tailwind `text-sm` maps to the interface role; `text-xs` maps to metadata. Use these for shared controls. Heading line heights and responsive layout belong in app.css. Do not resize the root font to change interface text: rem units also size the sidebar, spacing and controls.

## Color roles

Tokens provide light defaults and `.dark` overrides. Choose a role by meaning, not by its current hex value. Tokens with equal values may still represent different roles.

| Role | Tokens | Use |
| --- | --- | --- |
| Workspace | `--background`, `--foreground` | Main surface and readable text |
| Secondary surface | `--muted`, `--muted-foreground` | Hover areas and supporting text |
| Primary action | `--primary`, `--primary-foreground` | Main action and its label |
| Selection | `--accent`, `--accent-foreground` | Current destination or drop target |
| Secondary action | `--secondary`, `--secondary-foreground` | Lower-emphasis filled controls |
| Destructive action | `--destructive`, `--destructive-foreground` | Destructive actions and errors |
| Floating surface | `--popover`, `--popover-foreground` | Dialogs, menus and tooltips |
| Structure | `--border`, `--input`, `--ring` | Dividers, field boundaries and keyboard focus |
| Sidebar | `--sidebar-*` | Navigation-specific surface, text, selection and focus |

Use paired foreground/background roles together. Keep normal text at least 4.5:1 contrast and large text at least 3:1. A new state needs a usable light and dark treatment; reducing opacity alone is not a substitute for checking readability.

## Controls and surfaces

- Use the existing Button variants: default, outline, secondary, ghost, destructive and link. Keep keyboard focus visible.
- Use `--control-height` for ordinary desktop controls, `--control-height-touch` for coarse pointers and `--input-height` for text fields. A smaller glyph does not require a smaller touch target.
- Use `--icon-small` for header utility icons and `--icon-file` for file-type icons. Sidebar icon alignment is owned by the shared sidebar component.
- Use the `--radius` scale through the shared components. Workspace, header and footer corners should agree. File rows stay flat, with separators rather than enclosing cards.
- Use `--shadow-overlay` only for floating surfaces. Resting file and Settings content stays flat.
- Keep file previews large, with scrollable content and reachable header/footer controls.

## Required UI review rules

These rules apply to UI changes, including scoped exceptions below.

- Before implementation, compare the affected workflow in Nextcloud and at least one similar file manager. Inspect behavior and appearance using primary sources and screenshots or a visual walkthrough where available. Record the source, version or revision, inspection date, evidence and a short adopt/reject decision in the issue or PR. State when live access or visual evidence is unavailable; source reading alone is not a live comparison. A short note is enough.
- Use short action labels and familiar icons. Icon-only actions need accessible names and tooltips where their meaning needs clarification. Preserve visible keyboard focus and usable touch targets.
- Give routine success one compact status acknowledgement. For an all-success batch, show one count. Do not repeat a success heading, total and per-item prose for the same result. Put optional details behind a disclosure; expand details when needed to resolve mixed, failed or uncertain outcomes.
- Clear completed feedback on explicit refresh and navigation, including folder, search, pagination, destination and browser history changes. Automatic post-operation list revalidation must allow the completion to appear once. Returning to a view must not resurrect old results.
- Keep active work reachable across navigation. Separate feedback lifetime from operation state: clearing a notice must not cancel a transfer, discard an unresolved outcome or make retry unsafe. Never describe an uncertain result as success.
- Show pending work at the triggering control, with busy semantics and duplicate-request prevention. Reflect the actual request lifecycle, including failure; support reduced motion without hiding the busy state.
- Keep destructive consequences and recovery steps clear at the point where they matter. Compactness must not remove information needed to act safely.

| Workflow | Prefer | Avoid |
| --- | --- | --- |
| Move results | “Moved to Job” or “Moved 12 items”, with a named dismiss control | A success heading, 1-of-1 total, filename, source and destination prose for one move |
| Trash | Compact icon/name/location rows and a clear Restore action; parent/conflict guidance when relevant | Permanent instruction paragraphs, repeated “Original folder” labels and unnecessary raw references |
| Preview | Document space with a compact, named toolbar and reachable close/download controls | Repeated filename headings, explanatory chrome and unused padding that crowd the document |

Review feedback lifetime and visible pending states as part of the affected journey. Verify desktop and narrow layouts, both themes, keyboard access, and success/error/uncertain states that the change touches. Record the checks and their limits in the PR.

## Scoped exceptions

These are defaults, not a ban on variation. A scoped exception is appropriate when a surface has a different job: document content, a dense viewer toolbar or standalone sign-in branding can need a different treatment.

1. Try the existing role or component variant first.
2. Keep an exception local and explain the user need in the PR.
3. Check both themes, keyboard focus and the affected viewport sizes.
4. Promote a new shared role when the same intent recurs. Do not create a global token for every one-off measurement.

Equal numbers do not automatically mean the same rule. Layout widths, file-row geometry and document canvas colors remain local unless they encode a shared decision. PDF paper stays white because it represents document content.

## Changing the system

Update the owning token or shared component and its catalogue example together. Use the catalogue to compare normal, disabled, hover and keyboard-focus states. Then check the actual affected journey; a catalogue alone cannot prove application behavior.

Run Svelte checks and the affected browser tests, then rebuild `web/dist`. Required Linux CI remains the merge gate. New system changes use a separate PR; do not silently redesign unrelated surfaces during feature work.
