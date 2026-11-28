# Sylux

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Self-hosters and small groups. Account administration is occasional, rather than a daily team-management workflow.

## Product Purpose

Private file storage for small groups, with a browser interface for everyday file work: browsing, uploading, organizing, sharing and recovery.

## Product Direction

Build a pleasant, fast and efficient file server for small groups. Compete for the everyday file workflows small groups use in Nextcloud while keeping Sylux's operating overhead low. Favor clear workflows and measured resource use. Add capabilities that solve a concrete problem for these users; do not pursue broad feature parity for its own sake.

## Capabilities and Constraints

- Local accounts have administrator and user roles (`src/http/accounts.rs`).
- Files are private to each account by default. An owner can create revocable, expiring public links to individual files or live folders.
- Preserve existing upload reconciliation, replacement confirmation and recovery boundaries.
- Instance naming belongs in administrator Settings, persists on the server and applies to all users.
- Text and image previews are implemented in `web/src/lib/preview.ts`; PDF previews render locally, with page and zoom controls and a download fallback (`docs/pdf-preview.md`).
- Optional WebDAV access uses revocable app passwords scoped to the account's private libraries (`docs/webdav.md`).
- Optional [Keycloak sign-in](docs/oidc.md) uses administrator-approved account links and independent local recovery.
- Prototype data and account actions are simulated. The prototype is not deployment or durability evidence.

## Brand Commitments

Sylux is the default instance name. Use short, factual copy and a quiet, familiar file-manager interface. Remove promotional sign-in prose and unnecessary instruction from routine tasks.

## Product Principles

- Files, Trash and Account are separate destinations.
- Routine actions should be evident without explanatory paragraphs.
- Explain consequences where users could lose access or change stored files.
- Preserve keyboard access and clear interaction feedback.
- Treat responsiveness and resource use as part of the affected workflow. Make performance claims only when a named measurement supports them; scope claims to the measured host, release and workload.

## Evidence on Hand

The user's two annotated UI reviews and the review in `.impeccable/critique/2026-11-11T19-10-23Z__web-src-app-svelte.md`. The user approved the proposed direction and the small-group/admin-settings defaults. Representative prototype content is synthetic.
