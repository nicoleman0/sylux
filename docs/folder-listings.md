# Folder listings

In Files, activate **Name**, **Modified** or **Size** in the list header. Activate the same header again to reverse the direction. The default is name ascending. The current rows and direction stay in place until the new page arrives. A successful sort starts at the first page and clears the selection. If a request fails, the current order stays visible and the headers remain available. The committed sort carries through folder navigation and refreshes for the signed-in session.

`GET /folders/children` lists the root of the default Files library. `GET /folders/{id}/children` lists a folder. Both accept `sort=name_asc` (the default), `name_desc`, `size_asc`, `size_desc`, `modified_asc` or `modified_desc`, plus `limit` (1–500, default 100) and `cursor`.

Folders always come first, ordered by normalized name ascending. Files use the chosen name, byte-size or content-modified order. Size and modified-time ties use normalized name ascending, then entry ID ascending. Name ties use entry ID ascending. Names use case-sensitive NFC-normalized UTF-8 byte order, rather than locale-specific ordering. Folder sizes sum the current revisions of active files in each active descendant tree. Empty folders show 0 B; unavailable totals show “Size unknown”. Totals are calculated from committed metadata for each listing, without a stored cache. They do not affect folder order or cursor positions. Trash and search do not include folder totals.

`modified_at` is the current content revision's `committed_at`, in Unix seconds. First upload sets it; replacement uses the new revision's commit time. Rename, move, trash and restore preserve that revision time. Folders have no applicable modified time and return null. The current schema requires a non-null `committed_at` on every valid content revision. Older API responses may omit modified dates; the UI treats missing or invalid file dates as unknown. Unknown files follow known files in both modified modes. A file missing its content revision is invalid inventory and still fails loudly; the server does not invent a zero size or historical timestamp. Zero is a known timestamp. Files shows dates in the browser's local timezone, with the full local date and time in a tooltip and the existing Options menu. It does not infer a file's original filesystem date.

The server orders the complete listing before taking a page. Cursors are signed and bound to the account, folder, sort and folder generation. Reuse the same sort when requesting another page. Invalid sorts or mismatched cursors return HTTP 400. If the folder changes, the server returns `409 listing_changed`; restart from the first page. Listing cursors use protocol version 3. Older signed cursors are rejected after this update; use **Refresh** to restart if a cursor is no longer accepted.

Search results retain their separate [search ordering](search.md).

Browsing URLs identify My libraries, Files, Trash, Account or Settings. File browsing URLs include the folder ID and optional `q` and `sort` hash parameters. `GET /folders/{id}/path` resolves fresh library-to-folder descriptors for the signed-in owner's active ancestry. Missing, deleted or foreign ancestry returns 404 without folder names.

Folder and destination navigation add browser history entries. Search, sort and paging update the current entry. History keeps owner-bound cursor stacks and document scroll, including reload after signing in as the same owner. Entries and etags are fetched again; selection and completed notices are cleared. A copied URL starts on page one. Invalid or changed cursors restart on page one with a notice. An unavailable folder offers Files as a safe recovery destination.

Populated folders use [atomic tree trash and restore](trash.md). Hidden ancestry returns HTTP 404 for folder listings; restored descendants require fresh etags and cursors.

## Private libraries

`GET /libraries` returns `{items: [...]}` with the signed-in account's library root descriptors. The default library is named **Files** and appears first. Each descriptor includes `id`, `parent_id: null`, `name`, `kind: "folder"`, `generation`, and `etag`. The request returns `401 unauthorized` when the session is missing or invalid.

`POST /libraries` accepts `{"name":"Studio"}` and returns `201` with the new root descriptor. Use the session cookie, a matching Origin, and an `X-CSRF-Token` header. The endpoint returns these errors for a well-formed request.

| Response | When it occurs |
| --- | --- |
| `401 unauthorized` | The session is missing or invalid. |
| `403 forbidden` | The Origin or CSRF check fails. |
| `409 name_conflict` | The normalized name is already in use by one of the account's libraries. |
| `422 invalid_name` | The name fails entry-name validation. |
| `503 mutation_unavailable` | The server cannot admit storage mutations. |

Both endpoints can return `500 internal_error` if the server cannot complete the request.
