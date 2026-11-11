# UI review prototype

Mode: Operate. Code-first interactive prototype. The user approved the quiet file-manager direction and asked to review Files, sign-in and Account together. No further visual-choice round is needed for this prototype.

Process deviation: this prototype follows the user's accepted direction without a verified concept seed or QUALITY BAR card. The finish review assesses the rendered prototype against this brief; it does not establish comp fidelity or production approval.

## Direction contract

THESIS: Files are the main surface. Routine work uses a compact toolbar and clear destinations, without promotional prose or nested panels.

OWN-WORLD: System sans-serif, regular labels, medium-weight headings, restrained neutral surfaces, thin separators and one muted teal accent for primary/active states. Theme follows the system with a manual switch. Small radii; shadows only on menus/dialogs.

STORY: Sign in to an instance, browse a private folder, open/preview items, then reach personal Account or administrator Settings. The instance name is shared; personal identity lives in Account.

FIRST VIEWPORT: Slim full-width header; persistent narrow navigation at desktop; folder heading and breadcrumbs above a compact action row; filenames dominate aligned rows below. Sidebar and content sit on separate neutral planes, without a main enclosing card. Account and Settings retain the shell. Sign-in uses one focused form with a concise product description.

FORM: User-approved familiar file manager, extending Sylux's neutral theme and icon vocabulary. No concept lottery: the prior critique's direction was explicitly accepted. Signature interaction is clear destination change with active navigation, reversible history, focused headings and responsive row feedback. Transitions are brief and respect reduced motion.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

## Prototype scope

- Separate static application under `prototypes/ui-review/`, served only from that directory.
- Connected sign-in, Files, nested folders, Trash, Account, administrator Accounts and Instance Settings.
- Representative synthetic file/account data; no real backend or credential storage.
- Folder creation, preview, row menu actions, trash/restore and simulated account dialogs are reviewable.
- Instance name updates header, sign-in and document title in the prototype only.
- Clearly identified as a prototype. Password forms must say that no real password is changed; no credentials retained.
- Mobile navigation and layouts remain usable at 390px. Review desktop, mobile and 888px.
- Do not add fake storage-capacity numbers, security claims, unsupported commercial features or production API calls.
