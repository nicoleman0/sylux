CREATE TABLE library_publication (
    root_id TEXT PRIMARY KEY NOT NULL REFERENCES entry(entry_id),
    publication_id TEXT NOT NULL UNIQUE CHECK(length(publication_id)=32 AND publication_id NOT GLOB '*[^0-9a-f]*'),
    owner_account_id TEXT NOT NULL REFERENCES account(account_id)
) STRICT;

CREATE INDEX library_publication_owner ON library_publication(owner_account_id);

CREATE TRIGGER library_publication_identity BEFORE INSERT ON library_publication
WHEN NOT EXISTS(
    SELECT 1 FROM entry root
    JOIN account owner ON owner.account_id=root.owner_account_id AND owner.enabled=1
    WHERE root.entry_id=NEW.root_id AND root.owner_account_id=NEW.owner_account_id
      AND root.library_display_name IS NOT NULL AND root.state='active'
)
BEGIN
    SELECT RAISE(ABORT, 'invalid library publication');
END;

CREATE TRIGGER library_publication_disable AFTER UPDATE OF enabled ON account
WHEN OLD.enabled=1 AND NEW.enabled=0
BEGIN
    DELETE FROM library_publication WHERE owner_account_id=NEW.account_id;
END;

CREATE TRIGGER library_publication_trash AFTER UPDATE OF state ON entry
WHEN OLD.state='active' AND NEW.state='deleted' AND OLD.parent_id IS NULL
BEGIN
    DELETE FROM library_publication WHERE root_id=NEW.entry_id;
END;

UPDATE installation SET schema_version=7;
PRAGMA user_version=7;
