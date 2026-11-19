# Share a file

Open a file’s options menu and choose **Share**. Choose 1, 7 or 30 days, then select **Create link**. The default is 7 days. Copy the link before closing the dialog. Anyone who receives the link can download the file without signing in.

Use **Share** to see a file’s active links or **Account → Shared links** to see all your active links. Select **Revoke** to stop new downloads. Each link expires independently. Sylux shows the full URL only when it creates the link.

## When links stop working

- Expiry and revocation apply to the next request. A download already authorized may finish. Revocation has no effect on copies already downloaded.
- Replacing the file, moving it or any of its parent folders to Trash, or disabling the owner revokes affected links. Restoring files and re-enabling accounts do not bring old links back.
- Renaming or moving a live file keeps its links. A copy is a separate file with its own links.
- The recipient gets one attachment with the current filename. Sylux adds no surrounding folder path, account details, preview or listing. The file itself can contain information such as author metadata.

## Server contract

| Method and route | Contract |
| --- | --- |
| `POST /share-links` | Owner session, same Origin and CSRF token required. Submit `entry_id` and optionally `expiry_days`. Returns 201 with the link metadata and URL. |
| `GET /share-links` | Owner session required. Lists that account’s active links, with file name, link ID, entry ID, creation time and expiry. |
| `DELETE /share-links/{id}` | Owner session, same Origin and CSRF token required. Returns 204 for an authorized revoke, including a repeated revoke or a link outside the owner’s account. |
| `GET /share/{token}` | The bearer authorizes one full attachment with `no-store` and `no-referrer`. Unknown, expired, revoked, trashed and disabled-owner links return the same 404. |

The server issues a random 256-bit token and stores its SHA-256 verifier. Share routes do not emit URLs, request paths or tokens to application logs or lifecycle events. If you run a reverse proxy or CDN, redact the entire request target for `/share/` in access and error logs and leave caching disabled for those paths. Treat link URLs as credentials.

The server allows 50 active links per owner. Up to eight public download requests per server can hold a slot from metadata lookup through verified payload streaming. When all slots are busy, the server returns 429 with `Retry-After: 1`. A fresh request checks the link, account and file in one SQLite snapshot. Payload verification and streaming run outside SQLite.

Schema 3 adds the link table. Follow the [stopped-service backup and upgrade procedure](docker-beta.md#5-upgrade-and-roll-back) before starting a newer release. A restored backup reflects link status at backup time. Review shared links before reopening a restored instance to recipients.
