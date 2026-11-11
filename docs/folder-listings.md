# Folder listings

In Files, use **Sort files by** to choose name or file-size order. The default is **Name: A to Z**. The choice stays selected during folder navigation and refreshes for the signed-in session. Changing it starts at the first page and clears the current selection.

`GET /folders/children` lists the private root. `GET /folders/{id}/children` lists a folder. Both accept `sort=name_asc` (the default), `name_desc`, `size_asc` or `size_desc`, plus `limit` (1–500, default 100) and `cursor`.

Folders always come first, ordered by normalized name ascending. Files use the chosen name or byte-size order. Size ties use normalized name ascending, then entry ID ascending. Name ties use entry ID ascending. Names use case-sensitive NFC-normalized UTF-8 byte order, rather than locale-specific ordering. Folder sizes are not aggregated.

The server orders the complete listing before taking a page. Cursors are signed and bound to the account, folder, sort and folder generation. Reuse the same sort when requesting another page. Invalid sorts or mismatched cursors return HTTP 400. If the folder changes, the server returns `409 listing_changed`; restart from the first page. After a server update, use **Refresh** if a cursor is no longer accepted.

Search results retain their separate [search ordering](search.md). Modified-date sorting is not available.
