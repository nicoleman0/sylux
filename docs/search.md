# Account search

`GET /entries/search?q=...&limit=100&cursor=...` searches names in the signed-in account's active tree. It excludes Trash and the private root. Results include each entry and its containing folder path, from the root onward. The response has `items`, `next_cursor`, and `has_more`.

Matching is a literal substring after NFC normalization and Unicode lowercase conversion. `%` and `_` are ordinary characters. This rule does not apply full Unicode case folding or locale-specific casing. Results sort by the converted name, then the converted containing path, then entry ID.

The query must contain 1 to 255 UTF-8 bytes and no control characters. The page limit is 1 to 100, with 100 as the default. Cursors are signed, bounded, and tied to the account, exact query, and active tree state. A changed tree returns `409 listing_changed`; restart at the first page. Search reads the live tree for each request, so a new search reflects renames, moves, and deletions.
