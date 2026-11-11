---
name: Sylux
description: A quiet file manager for private, self-hosted storage.
colors:
  primary: "#146c60"
  primary-foreground: "#ffffff"
  background: "#ffffff"
  foreground: "#202b2b"
  secondary: "#e5f0ed"
  muted: "#f6f7f7"
  muted-foreground: "#5f6d68"
  destructive: "#a12d35"
  border: "#e3e8e7"
  input: "#d7dfdc"
  ring: "#277e71"
  dark-primary: "#8bd0bb"
  dark-primary-foreground: "#171c1c"
  dark-background: "#171c1c"
  dark-foreground: "#e4eae8"
  dark-secondary: "#283f36"
  dark-muted: "#1d2323"
  dark-muted-foreground: "#a2afab"
  dark-destructive: "#f0a1a5"
  dark-border: "#303b38"
  dark-input: "#42534b"
  dark-ring: "#8bd0bb"
typography:
  body:
    fontFamily: "system-ui, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif"
    fontSize: "14px"
    fontWeight: 400
  headline:
    fontFamily: "system-ui, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif"
    fontSize: "25px"
    fontWeight: 600
    lineHeight: 1.25
    letterSpacing: "-0.025em"
  title:
    fontFamily: "system-ui, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif"
    fontSize: "18px"
    fontWeight: 600
    letterSpacing: "-0.015em"
  label:
    fontFamily: "system-ui, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif"
    fontSize: "12px"
    fontWeight: 400
rounded:
  sm: "2px"
  control: "9px"
  dialog: "9px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "12px"
  lg: "16px"
  xl: "24px"
  2xl: "32px"
components:
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.primary-foreground}"
    rounded: "{rounded.control}"
    height: "32px"
  button-secondary:
    backgroundColor: "{colors.background}"
    textColor: "{colors.foreground}"
    rounded: "{rounded.control}"
    height: "32px"
  button-danger:
    backgroundColor: "{colors.destructive}"
    textColor: "#ffffff"
    rounded: "{rounded.control}"
    height: "32px"
  input:
    backgroundColor: "{colors.background}"
    textColor: "{colors.foreground}"
    rounded: "{rounded.control}"
    height: "43px"
  nav-active:
    backgroundColor: "{colors.secondary}"
    textColor: "{colors.primary}"
    rounded: "{rounded.control}"
    padding: "11px 14px"
---

# Design System: Sylux

## Overview

**Creative North Star: "The Quiet File Desk"**

Sylux gives stored files the visual priority. A slim header and compact destination navigation frame a broad working area; file names align in rows, while thin separators and restrained tonal shifts provide grouping. The result is familiar and low-noise for occasional file and account tasks.

The interface uses system sans-serif type, small control corners and a muted teal accent for actions and current locations. Light and dark themes share the same semantic roles. Copy stays short and factual, with consequences stated near account or file actions. The production design reflects the accepted file-manager direction; prototype documentation remains historical and separate.

**Key Characteristics:**
- Files occupy open, aligned rows instead of a large enclosing panel.
- Teal marks primary actions and active destinations; neutral planes carry the rest.
- The same shell supports file work, personal account details and administrator settings.

## Colors

Cool green-gray neutrals establish the workspace, with muted teal for actions and selection; dark mode keeps the same roles at higher contrast.

### Primary
- **Muted Teal** ({colors.primary}): Primary actions, links, folder emphasis and active navigation in the light theme.
- **Soft Mint Teal** ({colors.dark-primary}): The corresponding action and active-state color in dark mode.

### Neutral
- **White** ({colors.background}): Main light surface and input background.
- **Ink Green-Gray** ({colors.foreground}): Main light-theme text.
- **Pale Green-Gray** ({colors.secondary}): Light selection and accent surface.
- **Cool Mist** ({colors.muted}): Sidebar and quiet hover surface.
- **Muted Green-Gray** ({colors.muted-foreground}): Metadata and supporting text.
- **Quiet Line** ({colors.border}): Separators and control outlines.
- **Dark Charcoal** ({colors.dark-background}): Main dark surface.
- **Pale Gray-Mint** ({colors.dark-foreground}): Main dark-theme text.
- **Muted Red** ({colors.destructive}): Destructive actions and errors in light mode; dark mode uses {colors.dark-destructive}.

**The One Accent Rule.** Reserve teal for primary actions, links and the current destination; keep routine file surfaces neutral.

## Typography

**Display Font:** System UI sans-serif (system-ui, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif)
**Body Font:** System UI sans-serif (same stack)
**Label/Mono Font:** System UI sans-serif for interface labels; monospace is used for file content where appropriate.

**Character:** Compact, familiar interface typography. Semibold headings provide hierarchy without a separate display face.

### Hierarchy
- **Headline** (600, 25px, 1.25): Main page headings; 23px on narrow screens.
- **Title** (600, 18px): Settings and section headings.
- **Body** (400, 14px): Default interface text.
- **Label** (400, 12px): Compact metadata and list headings; some component labels use medium weight.
- **Brand** (600, 19px): Header wordmark, reduced to 18px on narrow screens.

**Not canonized:** The current build uses system UI for display headings. That implementation detail is recorded as observed, not prescribed for future display typography.

## Layout

The signed-in shell uses the shadcn-svelte sidebar-07 pattern: a collapsible icon rail beside an inset workspace with a 56px header. Below 768px, the header trigger opens navigation in a drawer. At 620px and below, content uses 17px side padding. File rows keep name, size and action columns aligned; the size column contracts on mobile. Account sections cap at 840px; administrator Settings cap at 680px. The beta notice sits within the workspace and follows its content insets. The common spacing rhythm includes 4, 8, 12, 16, 24 and 32px, with larger page insets for the main content.

**The Open Workspace Rule.** Let rows and section separators organize content; avoid enclosing the full file workspace in a card.

## Elevation & Depth

Most surfaces stay flat, separated by borders and tonal shifts. The shared shadow is reserved for floating menus and dialogs; resting file and settings areas have no elevation.

### Shadow Vocabulary
- **Overlay** (`0 8px 24px #132d2924`): Floating menus and dialog surfaces.

## Shapes

Controls and navigation share rounded corners from the radius scale. Buttons use 9px corners. File rows remain square and use thin bottom separators; small file-name controls use a 2px corner. Dialogs and workspace surfaces use 13px corners. The recurring focus treatment is a 2px outline with a 3px offset in the theme ring color.

## Components

### Buttons
- **Shape:** Rounded 9px radius and a 32px minimum height on desktop; 44px minimum for coarse pointers.
- **Primary:** Teal fill with theme-appropriate foreground; medium-weight text. Hover reduces fill intensity; pressed state shifts down by 1px.
- **Secondary:** Surface fill with a thin border; hover shifts to the muted surface.
- **Ghost:** No resting border or fill; hover uses the muted surface. Active navigation adds the selected surface and teal text/icon.
- **Danger:** Destructive text on a light destructive tint; disabled controls reduce opacity.
- **Focus:** Visible theme ring remains around interactive controls.

### Inputs / Fields
- **Style:** Theme surface, thin input border, compact corners and a 43px minimum height.
- **Focus / Error / Disabled:** Theme ring focus; invalid fields use destructive border and ring; disabled fields reduce opacity.

### Navigation
- **Desktop:** Collapsible sidebar with instance home link, Files, Trash and administrator Settings; account details sit at the bottom. Current location uses the selected surface and teal foreground.
- **Mobile:** Header trigger opens a navigation drawer, which closes after choosing a destination.

### File Rows
- **Style:** Flat rows with aligned name, size and action columns, 1px separators and a 59px minimum height on desktop.
- **Interaction:** Hover or focus-within adds a muted surface; active drag targets use the accent surface. Names truncate while metadata and actions retain their columns.

### Administrator Settings
- **Style:** Account and instance sections share the open content shell, section separators and restrained headings.
- **Behavior:** Settings tabs use a bottom rule with teal active text and underline. Compact account rows align identity, role/status and actions; explanatory copy remains close to consequential controls.

## Do's and Don'ts

### Do:
- **Do** keep file names visually dominant and actions grouped in the toolbar or row menu.
- **Do** use teal for primary actions and the current location.
- **Do** keep account and administrator settings in the same restrained workspace shell.
- **Do** state account and file consequences in concise nearby copy.

### Don't:
- **Don't** wrap the main file workspace in a large enclosing card.
- **Don't** add promotional sign-in copy, fake capacity figures or unsupported security claims.
- **Don't** add shadows to ordinary resting surfaces.
