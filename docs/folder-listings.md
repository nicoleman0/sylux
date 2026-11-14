# Folder listings

In Files, use **Sort files by** to choose name, file-size or modified-date order. The default is **Name: A to Z**. The choice stays selected during folder navigation and refreshes for the signed-in session. Changing it starts at the first page and clears the current selection.

`GET /folders/children` lists the private root. `GET /folders/{id}/children` lists a folder. Both accept `sort=name_asc` (the default), `name_desc`, `size_asc`, `size_desc`, `modified_asc` or `modified_desc`, plus `limit` (1–500, default 100) and `cursor`.

Folders always come first, ordered by normalized name ascending. Files use the chosen name, byte-size or content-modified order. Size and modified-time ties use normalized name ascending, then entry ID ascending. Name ties use entry ID ascending. Names use case-sensitive NFC-normalized UTF-8 byte order, rather than locale-specific ordering. Folder sizes sum the current revisions of active files in each active descendant tree. Empty folders show 0 B; unavailable totals show “Size unknown”. Totals are calculated from committed metadata for each listing, without a stored cache. They do not affect folder order or cursor positions. Trash and search do not include folder totals.

`modified_at` is the current content revision's `committed_at`, in Unix seconds. First upload sets it; replacement uses the new revision's commit time. Rename, move, trash and restore preserve that revision time. Folders have no applicable modified time and return null. The current schema requires a non-null `committed_at` on every valid content revision. Older API responses may omit modified dates; the UI treats missing or invalid file dates as unknown. Unknown files follow known files in both modified modes. A file missing its content revision is invalid inventory and still fails loudly; the server does not invent a zero size or historical timestamp. Zero is a known timestamp. Files shows dates in the browser's local timezone, with the full local date and time in a tooltip and the existing Options menu. It does not infer a file's original filesystem date.

The server orders the complete listing before taking a page. Cursors are signed and bound to the account, folder, sort and folder generation. Reuse the same sort when requesting another page. Invalid sorts or mismatched cursors return HTTP 400. If the folder changes, the server returns `409 listing_changed`; restart from the first page. Listing cursors use protocol version 3. Older signed cursors are rejected after this update; use **Refresh** to restart if a cursor is no longer accepted.

Search results retain their separate [search ordering](search.md). Search has no modified-date metadata or sorting.

Browsing URLs identify Files, Trash, Account or Settings. Files URLs include the folder ID and optional `q` and `sort` hash parameters. `GET /folders/{id}/path` resolves fresh root-to-folder descriptors for the signed-in owner's active ancestry. Missing, deleted or foreign ancestry returns 404 without folder names.

Folder and destination navigation add browser history entries. Search, sort and paging update the current entry. History keeps owner-bound cursor stacks and document scroll, including reload after signing in as the same owner. Entries and etags are fetched again; selection and completed notices are cleared. A copied URL starts on page one. Invalid or changed cursors restart on page one with a notice. An unavailable folder offers Files as a safe recovery destination.

Populated folders use [atomic tree trash and restore](trash.md). Hidden ancestry returns HTTP 404 for folder listings; restored descendants require fresh etags and cursors.
