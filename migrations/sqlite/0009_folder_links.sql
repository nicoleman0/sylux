-- A null revision identifies a live folder link. File links retain their original revision.
DROP TRIGGER share_link_disable;
DROP TRIGGER share_link_replace;
DROP TRIGGER share_link_trash;
ALTER TABLE share_link RENAME TO previous_share_link;

CREATE TABLE share_link (
    share_id TEXT PRIMARY KEY NOT NULL CHECK(length(share_id)=32 AND share_id NOT GLOB '*[^0-9a-f]*'),
    verifier BLOB NOT NULL UNIQUE CHECK(length(verifier)=32),
    owner_account_id TEXT NOT NULL REFERENCES account(account_id),
    entry_id TEXT NOT NULL REFERENCES entry(entry_id),
    revision_id TEXT REFERENCES revision(revision_id),
    created_at INTEGER NOT NULL CHECK(created_at>=0),
    expires_at INTEGER NOT NULL CHECK(expires_at-created_at IN (86400,604800,2592000))
) STRICT;
INSERT INTO share_link(share_id, verifier, owner_account_id, entry_id, revision_id, created_at, expires_at)
SELECT share_id, verifier, owner_account_id, entry_id, revision_id, created_at, expires_at FROM previous_share_link;
DROP TABLE previous_share_link;

CREATE INDEX share_link_owner ON share_link(owner_account_id, expires_at);
CREATE INDEX share_link_entry ON share_link(entry_id);
CREATE INDEX share_link_expiry ON share_link(expires_at);

CREATE TRIGGER share_link_disable AFTER UPDATE OF enabled ON account
WHEN OLD.enabled=1 AND NEW.enabled=0
BEGIN
    DELETE FROM share_link WHERE owner_account_id=NEW.account_id;
END;

CREATE TRIGGER share_link_replace AFTER UPDATE OF current_revision_id ON entry
WHEN OLD.current_revision_id IS NOT NULL AND OLD.current_revision_id IS NOT NEW.current_revision_id
BEGIN
    DELETE FROM share_link WHERE entry_id=NEW.entry_id AND revision_id IS NOT NULL;
END;

CREATE TRIGGER share_link_trash AFTER UPDATE OF state ON entry
WHEN OLD.state='active' AND NEW.state='deleted'
     AND EXISTS(SELECT 1 FROM share_link WHERE owner_account_id=NEW.owner_account_id)
BEGIN
    DELETE FROM share_link WHERE owner_account_id=NEW.owner_account_id AND entry_id IN (
        WITH RECURSIVE tree(entry_id,kind) AS (
            SELECT NEW.entry_id,NEW.kind
            UNION ALL
            SELECT child.entry_id,child.kind FROM tree JOIN entry child ON child.parent_id=tree.entry_id
            WHERE tree.kind='folder' AND child.owner_account_id=NEW.owner_account_id AND child.state='active'
        )
        SELECT entry_id FROM tree
    );
END;

CREATE TRIGGER share_link_identity BEFORE INSERT ON share_link
WHEN NOT EXISTS (
    SELECT 1 FROM entry e JOIN account a ON a.account_id=e.owner_account_id AND a.enabled=1
    WHERE e.entry_id=NEW.entry_id AND e.owner_account_id=NEW.owner_account_id AND e.state='active'
    AND ((e.kind='folder' AND NEW.revision_id IS NULL)
         OR (e.kind='file' AND e.current_revision_id=NEW.revision_id))
)
BEGIN
    SELECT RAISE(ABORT, 'invalid share link target');
END;

UPDATE installation SET schema_version=9;
PRAGMA user_version=9;
