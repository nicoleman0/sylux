# Trash and restore

Move a file or populated folder to Trash, then use **Restore** to return it to its original parent and name. A missing or hidden parent, or a name conflict, blocks restoration. Restore the parent first or rename the conflicting active item. Account roots cannot be moved to Trash. When a selection contains a folder and an item inside it, the folder action covers both.

A folder operation is one atomic metadata transaction. Only the selected root becomes deleted; active descendants remain stored behind it. Files, search, direct content requests and mutations require an active same-owner path to the private root. Restoring the folder makes its active tree reachable again, with the same structure, revisions and bytes. Earlier deletions stay separate in Trash and cannot be restored through a hidden ancestor.

Trashing increments generations on the selected root, its active reachable descendants and its original parent. Traversal stops at earlier deletions. Restoring increments the selected root and its parent. Old descendant etags, listing cursors and upload parent preconditions stay stale after a trash/restore cycle. Writer order determines which moves and upload commits finish before the tree is hidden; later effects fail their current ancestry or generation check. Existing upload admission replay, status, cancellation and reconciliation remain available.

Namespace operations have no durable operation ID. After a network failure or server error, the outcome is unconfirmed. Inspect fresh Files and Trash before acting again. Inspection can establish current state, but does not prove that a particular request ran exactly once. If the original parent is unavailable, inspect Trash and dismiss the unconfirmed result to recover the parent before checking the item again. Never automatically retry a trash or restore effect after an unknown reply.

Payloads and revisions are retained. This feature does not add permanent removal, retention limits, schema migration or durability qualification.
