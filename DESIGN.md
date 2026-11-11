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

## Flexible defaults

These are defaults, not a ban on variation. A scoped exception is appropriate when a surface has a different job: document content, a dense viewer toolbar or standalone sign-in branding can need a different treatment.

1. Try the existing role or component variant first.
2. Keep an exception local and explain the user need in the PR.
3. Check both themes, keyboard focus and the affected viewport sizes.
4. Promote a new shared role when the same intent recurs. Do not create a global token for every one-off measurement.

Equal numbers do not automatically mean the same rule. Layout widths, file-row geometry and document canvas colors remain local unless they encode a shared decision. PDF paper stays white because it represents document content.

## Changing the system

Update the owning token or shared component and its catalogue example together. Use the catalogue to compare normal, disabled, hover and keyboard-focus states. Then check the actual affected journey; a catalogue alone cannot prove application behavior.

Run Svelte checks and the affected browser tests, then rebuild `web/dist`. Required Linux CI remains the merge gate. New system changes use a separate PR; do not silently redesign unrelated surfaces during feature work.
