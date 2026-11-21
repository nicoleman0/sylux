# Libraries and sharing

## Why

People need to organize their files into named libraries and share selected
content with other accounts, the signed-in instance or external recipients.
Private storage remains the default. See the [product direction](../../PRODUCT.md).

## Constraints

- Linear owns delivery scope, priorities and work state. This document records
  behavior. Each item marked **Nick approval required** is a recommendation
  awaiting a decision. Record Nick's decisions in Linear and update the
  affected rules and scenarios before the implementation issues start.
- Scenarios start as **planned**. Behavior tests cite their stable scenario IDs;
  the PR adding a test records the scenario as verified.

### Shipped single-file links

The [single-file link contract](../sharing.md#server-contract) remains in force.
An active bearer URL authorizes one attachment for one file revision without a
session. It gives no access to folders, other files or account information.
Keep existing URLs, ownership, file and revision identity, expiry and
revocations through migration. A live rename or move preserves the link.
Replacement, Trash of the file or an ancestor, and owner disablement revoke it.
Restoring the file or re-enabling the owner cannot revive it. The
[operator procedure for restoring a backup](../sharing.md#server-contract) still
applies.

## Capabilities

### Libraries and migration

[SYL-110](https://linear.app/corvus-dev/issue/SYL-110/create-named-libraries-and-browse-existing-files-from-my-libraries)
owns library identity, ownership, migration and My libraries.

- **Rule.** An owner can create and browse named libraries with distinct
  identities. Existing files remain accessible to their owner after upgrade.
  New and migrated content is private by default. Migration creates no grants
  or folder links and preserves the shipped single-file link contract.
- **Nick approval required.** Show existing files in one private library per
  account. Retain the existing hierarchy without asking owners to rearrange it.
- **LIB-01.** Create two libraries with identically named child folders. Use
  the keyboard to open each, reload it and reopen its saved URL. Each shows
  its own contents. A second account sees neither library in navigation, search
  or direct requests.
- **LIB-02.** Upgrade accounts with existing folders, files, and active, expired
  and revoked file links. Owners can browse their original hierarchy in one
  private library per account, and ownership is unchanged. Active URLs still
  serve the same revisions until expiry or
  revocation. Old expired and revoked URLs still fail. The upgrade creates no
  new access for another account or for an anonymous visitor.

### Direct account grants

[SYL-111](https://linear.app/corvus-dev/issue/SYL-111/share-a-library-or-folder-with-another-account)
owns direct grants, Shared with me and direct-grant revocation.

- **Rule.** An owner can grant a named account access to a library or subfolder
  and revoke that grant. A recipient can discover the root in Shared with me.
  The server checks current access for every request, including listings,
  metadata, deep URLs, downloads and search. Results and navigation expose only
  authorized content, including only authorized path segments.
- **Nick approval required.** Start with owner-managed, read-only grants to
  enabled accounts. Allow browsing, search and download within the granted
  subtree. Reserve upload, replacement, rename, move, copy into the subtree,
  Trash, restore and sharing controls for the owner. Deny server-side copies by
  recipients. Grant no extra access based on administrator status.
- **Nick approval required.** Allow PDF previews within the granted subtree,
  checking current access on each new request. Defer shared ZIP creation and
  download for non-owners in this slice; preserve existing owner ZIP behavior.
- **Nick approval required.** Disabling the owner or named recipient revokes
  affected direct grants. Re-enabling either account requires a fresh grant.
- **DIR-01.** Share a library and a separate nested folder with a named
  recipient. Check discovery, nested URLs, downloads and search as owner,
  recipient and unrelated account. The recipient can read within the granted
  roots and preview PDFs, but cannot learn private parents or siblings, change
  stored content, copy files server-side into their own storage, create shares
  or create or download a shared ZIP. An unrelated account, including an
  administrator, gets no access. The owner retains existing ZIP behavior.
- **DIR-02.** Revoke a grant with no other access in place. The recipient loses
  discovery, search, listings and new requests to saved URLs and PDF previews.
  Repeat with owner disablement and with recipient disablement. Re-enabling either account
  leaves the grant revoked.

### Instance publication

[SYL-112](https://linear.app/corvus-dev/issue/SYL-112/publish-a-library-to-everyone-signed-into-the-instance)
owns library publication, withdrawal and Shared with all.

- **Rule.** An authorized publisher can publish or withdraw a whole library.
  Publication gives signed-in accounts access through Shared with all, subject
  to the same server-side checks as a direct grant. Publication alone gives
  anonymous visitors no access. Private sibling libraries remain private.
- **Nick approval required.** Let only an enabled library owner publish and
  withdraw their library. Grant read-only access to all enabled signed-in
  accounts, including future accounts. Disabling the owner withdraws the
  publication. Re-enabling the owner requires explicit republication.
- **ALL-01.** Publish one of two libraries. Two signed-in accounts can find,
  search, open and download from the published library, including by deep URL.
  Repeat with an account created after publication. Anonymous requests,
  recipient mutations, shared ZIP creation and download, and requests for the
  private sibling are denied. A recipient can preview an in-scope PDF; a
  non-owner, including an administrator, cannot publish or withdraw the
  library. Owner ZIP behavior remains available. Check withdrawal and owner disablement separately. Without an independent grant,
  new saved URL, download and PDF preview requests are denied and discovery is
  gone. Re-enabling the owner leaves the publication withdrawn.

### Overlapping grants

This rule is shared by [Direct account grants](#direct-account-grants) and
[Instance publication](#instance-publication). Integration assumes both grant
modes exist. SYL-112 is accountable for approval evidence for OVR-01 and
OVR-02; schedule SYL-111 before SYL-112 verification and closeout.

- **Rule.** Each grant is independent. Revoking a direct grant or withdrawing
  a publication removes only the access it supplied. Current access can still
  come from another direct or instance grant. Recheck discovery, search and
  requests against the remaining scope.
- **OVR-01.** Publish a library and separately grant one account access to a
  nested folder. Withdraw publication. That account can still open the folder
  from Shared with me and use its saved URLs, but loses access to the rest of
  the library. The other account loses access entirely. Revoke the last grant
  and check that the first account also loses access.
- **OVR-02.** With both grants active again, revoke the direct grant first.
  The recipient can still find and open the library, including the folder,
  through Shared with all. Withdraw publication and verify that discovery,
  search and new direct requests now fail.
- **OVR-03.** Grant the same account a library and a nested folder directly.
  Revoke the library grant. The nested grant still works and the rest of the
  library is private again.

### Moves and Trash

Apply these cases to each sharing mechanism within its issue scope. The
[shipped single-file rules](#shipped-single-file-links) apply independently.

- **Nick approval required.** Evaluate inherited access against the current
  live subtree. A rename or an allowed move within the same owner's storage
  keeps a share attached to its root. Moving content out ends inherited access.
  Moving content in gives the destination's audience access. Before a move
  exposes content to a new audience or carries a share into a new location,
  show the effect and require owner confirmation.
- **Nick approval required.** Trashing a shared root or an ancestor revokes
  grants and folder links rooted in that subtree and withdraws publications
  there. Restore never recreates those shares. Trashing only a descendant
  leaves the ancestor's share active and removes the descendant from access.
  Before restoring into an active shared tree, show the audience and require
  owner confirmation. A confirmed restore re-enters the live ancestor share.
- **TREE-01.** Move a nested file and folder out of, then into, a shared tree.
  Check that listings, search where supported, saved URLs and downloads follow
  the new boundary. Cancel the move into the shared tree first and check that
  the content stays private. For a movable shared root, rename or move it
  within the owner's storage and verify that its share follows the root
  without exposing new ancestors.
- **TREE-02.** Trash a descendant within a shared tree. Other contents stay
  available. Cancel a restore into that tree and check that the item stays
  hidden. Confirm the restore and check inherited access. Trash and restore
  the shared root. For a nested shared root, repeat by trashing and restoring
  an ancestor. Old shares stay revoked and must be explicitly recreated.

### Anonymous folder links

[SYL-113](https://linear.app/corvus-dev/issue/SYL-113/share-a-folder-externally-through-an-expiring-public-link)
owns the proposed folder-link extension and reuses owner link management.

- **Rule.** An expiring bearer URL lets a signed-out recipient browse a folder
  and its allowed descendants and download individual files. Every listing and
  download request checks the current token and subtree boundary, even if a
  session is present. The link exposes no private ancestors, siblings or
  account details and gives no authenticated grant or mutation rights. Keep
  recipient request paths, URLs and tokens out of application logs and
  lifecycle events. Redact recipient request targets at the proxy. Preserve
  existing single-file links.
- **Nick approval required.** Let only the enabled folder owner create and
  revoke links. Reuse 1, 7 and 30 day expiries with 7 days as default. Use a
  live subtree, so replacing a descendant makes its new revision available
  through the folder link. Make that audience visible before replacement.
  Limit anonymous access to browsing and individual downloads. Exclude
  anonymous search, previews and archive downloads. SYL-113 must decide whether
  folder links count toward the existing owner link quota and whether anonymous
  listings consume public download slots.
- **Nick approval required.** Revocation and expiry deny the next request.
  Owner disablement permanently revokes the link. Already authorized downloads
  may finish. Apply the same no-store, no-referrer and uniform denial behavior
  as file links. Include folder links in the backup restore procedure for
  restricting recipient access while owners review links.
- **PUB-01.** Create a folder link and open it signed out. Browse and download
  a nested file. Direct requests outside the subtree and requests to authenticated
  routes, search, previews, archives or mutations get no access from the token.
  Repeat the outside-subtree request with a signed-in session on the public route.
  Check the approved descendant replacement rule and the move and Trash cases
  on public requests. Verify that a separate single-file link retains its
  documented behavior.
- **PUB-02.** Create separate links and check explicit revocation, expiry and
  owner disablement against fresh listings and saved download URLs. Check that
  revoking one link leaves another active, that an authorized download can
  finish, and that re-enabling the owner leaves revoked URLs unusable. Check
  response headers and application and proxy log handling with disposable
  credentials. Compare out-of-subtree, revoked, expired and disabled-owner
  requests for the same uniform denial behavior.

## Non-goals

This contract excludes cross-owner transfers, collaborative editing, group
roles and site-wide anonymous discovery.

## Success signal

Each implementation proves its applicable scenarios with owner, recipient,
unrelated and signed-out requests as relevant. A PR that changes specified
behavior updates the rule, scenario and behavior test together. Reviewers
verify both directions: each changed rule has a matching scenario and test,
and each changed behavior test agrees with the rule it checks. A refactor that
preserves behavior can cite existing passing coverage.
