---
name: Sylux UI Review Prototype
description: A quiet, responsive file manager prototype with a restrained teal accent.
colors:
  accent: "#146c60"
  accent-hover: "#0d594f"
  active: "#e5f0ed"
  bg: "#ffffff"
  side: "#f6f7f7"
  text: "#202b2b"
  muted: "#667171"
  line: "#e3e8e7"
  hover: "#f1f5f4"
  danger: "#a12d35"
typography:
  body:
    fontFamily: "system-ui, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.5
  title:
    fontFamily: "system-ui, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif"
    fontSize: "25px"
    fontWeight: 600
    lineHeight: 1.25
    letterSpacing: "-0.025em"
  label:
    fontFamily: "system-ui, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif"
    fontSize: "12px"
    fontWeight: 400
rounded:
  sm: "4px"
  md: "5px"
  lg: "7px"
  dialog: "9px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "14px"
  lg: "24px"
  xl: "44px"
components:
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.bg}"
    rounded: "{rounded.md}"
    padding: "0 14px"
    height: "44px"
  button-secondary:
    backgroundColor: "{colors.bg}"
    textColor: "{colors.text}"
    rounded: "{rounded.md}"
    padding: "0 14px"
    height: "44px"
  input:
    backgroundColor: "{colors.bg}"
    textColor: "{colors.text}"
    rounded: "{rounded.md}"
    padding: "9px 12px"
    height: "43px"
  nav-active:
    backgroundColor: "{colors.active}"
    textColor: "{colors.accent}"
    rounded: "{rounded.md}"
    padding: "11px 14px"
    height: "44px"
---

# Design System: Sylux UI Review Prototype

## Overview

**Creative North Star: "The Quiet File Desk"**

This system gives files the main visual weight. A slim header and compact navigation frame a broad working area; filenames sit in aligned rows, with thin separators doing the grouping. Neutral surfaces and a single muted teal accent keep routine file work calm and familiar.

The system uses system sans-serif type, small radii and brief state transitions. Light and dark themes share the same roles; the theme follows the operating system unless the user chooses one manually. This is a prototype direction pending user review, not approval of Sylux's production design system.

**Key Characteristics:**
- Quiet neutral planes with teal reserved for primary and active states.
- Rows, separators and navigation carry structure instead of enclosing cards.
- Responsive shell and reduced-motion support.

## Colors

The light theme uses white and cool near-white surfaces with dark green-gray text; muted teal marks action and selection.

### Primary
- **Muted Teal** (#146c60): Primary actions, links and active navigation in the light theme.
- **Deep Teal** (#0d594f): Hover state for primary actions.

### Neutral
- **White** (#ffffff): Main and input surface; also the light button text color.
- **Soft Gray** (#f6f7f7): Sidebar and secondary surface.
- **Ink Green-Gray** (#202b2b): Main text.
- **Muted Green-Gray** (#667171): Secondary text, icons and metadata.
- **Quiet Line** (#e3e8e7): Separators and control borders.
- **Row Wash** (#f1f5f4): Hover surface.
- **Selected Wash** (#e5f0ed): Active navigation and selected folder treatment.
- **Muted Red** (#a12d35): Destructive actions and errors.

### Named Rules
**The One Accent Rule.** Keep teal for primary actions, links and current locations; use neutral surfaces for the rest of the file workspace.

## Typography

**Display Font:** System UI sans-serif stack.
**Body Font:** System UI sans-serif stack.
**Label/Mono Font:** System UI sans-serif for labels; monospace is reserved for previewed file text.

**Character:** Compact, familiar interface typography. Medium-weight headings provide hierarchy without introducing a separate display face.

### Hierarchy
- **Title** (600, 25px, 1.25): Page headings; 23px on narrow screens.
- **Section title** (600, 18px): Settings and section headings.
- **Body** (400, 14px, 1.5): Default interface text.
- **Label** (400 or 500, 12px): Metadata and form labels; muted when secondary.
- **File preview** (400, 13px/1.8): Monospace sample content.

## Layout

Desktop uses a 212px sidebar beside a flexible content column, under a 65px sticky header. At widths below 1000px, the sidebar narrows to 180px. At 620px and below, it becomes a sticky horizontal navigation under a 59px header; page padding contracts to 17px and file rows retain aligned name, size and action columns. Main content begins at 43px top and 48px sides on desktop, with a 60px side inset at very wide widths. Settings and Account content cap at 840px. The recurring spacing steps include 4, 8, 14, 24 and 44px, with larger page and section gaps where needed.

## Elevation & Depth

Most surfaces remain flat. Borders and tonal shifts separate the sidebar, rows and controls. Menus and dialogs use shadows for temporary overlay depth; the toast shares the menu shadow. No resting card shadow is used.

### Shadow Vocabulary
- **Overlay menu:** `0 8px 30px #132d2924` in light theme; `0 10px 32px #0006` in dark theme.
- **Dialog:** `0 18px 70px #0003`.

## Shapes

Controls and navigation use compact 5px corners; skip link and badges use 4px, menus 7px, and dialogs 9px. File rows are square-edged, separated by hairline borders. The profile avatar is circular. Focus uses a 2px outline with 3px offset in the theme focus color.

## Components

### Buttons
- **Shape:** Compact 5px radius; minimum height 44px.
- **Primary:** Teal fill, white/light-theme text, 14px horizontal padding; hover deepens the teal.
- **Secondary:** Surface fill, thin line border, 14px horizontal padding; hover uses the row wash and a stronger border.
- **Danger:** Red text and border; hover adds a pale red wash.
- **Focus / active:** Interactive controls retain a visible 2px focus outline; pressed buttons darken slightly. Route headings and main regions receive programmatic focus without an outline.

### Inputs / Fields
- **Style:** Theme input surface, 1px line border, 5px radius, 9px 12px padding and 43px minimum height.
- **Focus:** 2px theme-colored outline offset by 3px; hover strengthens the border.
- **Error:** Muted red border and message.

### Navigation
- **Desktop:** Vertical sidebar links with 11px 14px padding; current destination uses selected wash and teal text/icon.
- **Mobile:** Sticky horizontal row below the header; account/settings utilities reduce to icon controls.
- **Interaction:** Hover uses row wash. Focus remains visible.

### File Rows
- **Style:** Flat, 59px minimum rows separated by thin lines; folder icon and active row use teal.
- **Interaction:** Hover or focus-within adds row wash; filenames truncate with an ellipsis while size and action columns remain aligned.

### Menus and Dialogs
- **Menu:** 178px minimum width, 7px radius, thin border, compact padding and overlay shadow; items have 44px minimum height.
- **Dialog:** 440px default width, 9px radius, thin border and broad shadow; preview dialog expands to 720px.

## Do's and Don'ts

### Do:
- **Do** keep file names visually dominant and actions grouped in the toolbar or row menu.
- **Do** use teal for primary actions and the current location.
- **Do** keep focus outlines visible and honor reduced-motion preferences.
- **Do** let neutral borders and surfaces establish grouping.

### Don't:
- **Don't** wrap the main file workspace in a large enclosing card.
- **Don't** add promotional sign-in copy or unsupported capacity claims.
- **Don't** use shadows on ordinary resting surfaces.
