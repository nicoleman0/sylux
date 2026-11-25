CREATE TABLE direct_grant (
    grant_id TEXT PRIMARY KEY NOT NULL CHECK(length(grant_id)=32 AND grant_id NOT GLOB '*[^0-9a-f]*'),
    owner_account_id TEXT NOT NULL REFERENCES account(account_id),
    recipient_account_id TEXT NOT NULL REFERENCES account(account_id),
    root_id TEXT NOT NULL REFERENCES entry(entry_id),
    CHECK(owner_account_id <> recipient_account_id),
    UNIQUE(root_id, recipient_account_id)
) STRICT;
CREATE INDEX direct_grant_recipient ON direct_grant(recipient_account_id, root_id);
CREATE INDEX direct_grant_owner ON direct_grant(owner_account_id, root_id);

CREATE TRIGGER direct_grant_identity BEFORE INSERT ON direct_grant
WHEN NOT EXISTS(
    SELECT 1 FROM entry root
    JOIN account owner ON owner.account_id=root.owner_account_id AND owner.enabled=1
    JOIN account recipient ON recipient.account_id=NEW.recipient_account_id AND recipient.enabled=1
    WHERE root.entry_id=NEW.root_id AND root.owner_account_id=NEW.owner_account_id
      AND root.kind='folder' AND root.state='active'
)
BEGIN
    SELECT RAISE(ABORT, 'invalid direct grant');
END;

CREATE TRIGGER direct_grant_disable AFTER UPDATE OF enabled ON account
WHEN OLD.enabled=1 AND NEW.enabled=0
BEGIN
    DELETE FROM direct_grant WHERE owner_account_id=NEW.account_id;
    DELETE FROM direct_grant WHERE recipient_account_id=NEW.account_id;
END;

CREATE TRIGGER direct_grant_trash AFTER UPDATE OF state ON entry
WHEN OLD.state='active' AND NEW.state='deleted'
     AND EXISTS(SELECT 1 FROM direct_grant WHERE owner_account_id=NEW.owner_account_id)
BEGIN
    DELETE FROM direct_grant WHERE owner_account_id=NEW.owner_account_id AND root_id IN (
        WITH RECURSIVE tree(entry_id) AS (
            SELECT NEW.entry_id
            UNION ALL
            SELECT child.entry_id FROM tree
            JOIN entry child ON child.parent_id=tree.entry_id
            WHERE child.owner_account_id=NEW.owner_account_id AND child.state='active' AND child.kind='folder'
        )
        SELECT entry_id FROM tree
    );
END;

UPDATE installation SET schema_version=6;
PRAGMA user_version=6;
