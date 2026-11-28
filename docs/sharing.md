# Share files and folders with a public link

Open an item's options menu. For a file, choose **Share**. For a folder, choose **Public link**. You can also use **Public link** beside a library in **My libraries**. Choose 1, 7 or 30 days and select **Create link**. The default is 7 days. Copy the address before closing the dialog. Sylux shows the full URL only at creation.

Anyone with the address can use it without signing in. A file link downloads one file. A folder link lets recipients browse the shared folder and its current subfolders and download individual files. The recipient view ends at the shared root, even in a signed-in browser. It offers no search, previews, archives or changes to files. File contents can still include information such as author metadata.

Use the item's link dialog or **Account → Shared links** to review and revoke your active links. Direct sharing with other accounts has its own controls.

## Live folder contents

- Add, copy, move in, restore or replace content to make it available to anyone with an active public link to the destination. The web app warns at the operation. Moving or restoring into a newly shared location requires confirmation. Cancel to leave the item in place or in Trash.
- Moving items out or sending them to Trash makes them inaccessible through that folder link, including from saved descendant URLs. The rest of the folder remains available. Restoring a descendant into the shared tree makes it visible again.
- Replacement uploads create a new file entry. Recipients reopen the listing to get its new download address. The original file's standalone links are revoked.
- Renaming or moving the shared root keeps its link working. Recipients see its current name and only the path from that root.
- WebDAV uploads, replacements and new folders become visible through active folder links. Use the web app to confirm moves that introduce a new audience. Owners can open **Public link active** on a shared folder to revoke its links.

## When links stop working

- Expiry and revocation apply to the next request. An authorized download may finish. Copies already downloaded stay with recipients.
- Sending the shared item or any ancestor to Trash, or disabling its owner, permanently revokes affected links. Restoring those items or enabling the owner again leaves the old URLs revoked.
- Replacing a directly shared file revokes its file links. Renaming or moving a live file keeps them. A copy is a separate item with its own links.

## Server and proxy handling

| Method and route | Contract |
| --- | --- |
| `POST /share-links` | Enabled owner session, matching Origin and CSRF token. Submit `entry_id` and optionally `expiry_days`. Returns 201 with link metadata and the URL. |
| `GET /share-links` | Owner session. Lists active file and folder links with kind, name, link ID, entry ID, creation time and expiry. |
| `DELETE /share-links/{id}` | Owner session, matching Origin and CSRF token. Returns 204 for an authorized revoke, including repeated requests and IDs outside the owner's account. |
| `GET /share/{token}` | One full attachment for an existing file link. |
| `GET /share/{token}/browse` | Folder link landing page. |
| `GET /share/{token}/folders/{id}` | Browse a folder in the live subtree. |
| `GET /share/{token}/files/{id}` | Download one file in the live subtree as a full attachment. |

Every public response uses `Cache-Control: no-store` and `Referrer-Policy: no-referrer`. Unknown, expired, revoked, out-of-scope and unavailable targets get the same 404. Each request checks the owner, link and current ancestry in one SQLite snapshot. Payload verification and streaming run outside SQLite.

The 50 active links per owner include files and folders. File downloads share eight slots per server. Each request holds one from metadata lookup until verified streaming completes or disconnects. If all slots are occupied, new download requests receive 429 with `Retry-After: 1`. Folder browsing and owner controls remain available.

Sylux issues a random 256-bit token and stores its SHA-256 verifier. Treat the full URL as a credential. Share routes omit URLs, request targets and tokens from application logs and lifecycle events. At a reverse proxy or CDN, pass through the configured public origin and keep caching disabled for `/share` and all `/share/` paths. Suppress or redact the whole request target, including its query, in access and error logs, tracing and analytics for those paths. Exclude cookies, referrers and share-management response bodies from those logs. Test the logging configuration with disposable links before exposing the service.

## Backup and restore

Schema 9 extends the link table and preserves existing file links. Follow the [stopped-service backup and upgrade procedure](docker-beta.md#5-upgrade-and-roll-back) before starting a newer release.

A restored snapshot can revive unexpired links that owners revoked after the snapshot. Folder links can expose the subtree as it existed in that snapshot. Before restoring, restrict recipient access at the proxy or network, including all `/share/` paths. Keep access restricted while owners review and revoke links in **Account → Shared links**. If owners cannot review their links, keep the restriction. Keep backup files private.
