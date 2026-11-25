-- Keep the original root ID as the default library for every account.
ALTER TABLE entry ADD COLUMN library_name TEXT
    CHECK (library_name IS NULL OR (parent_id IS NULL AND kind = 'folder'
        AND length(CAST(library_name AS BLOB)) BETWEEN 1 AND 255));
ALTER TABLE entry ADD COLUMN library_name_key BLOB
    CHECK ((library_name_key IS NULL AND library_name IS NULL)
        OR (library_name_key IS NOT NULL AND library_name IS NOT NULL
            AND length(library_name_key) BETWEEN 1 AND 255));
ALTER TABLE entry ADD COLUMN library_display_name TEXT GENERATED ALWAYS AS (
    CASE WHEN parent_id IS NULL AND kind = 'folder' THEN coalesce(library_name, 'Files') END
) VIRTUAL;

DROP INDEX entry_one_root;
CREATE UNIQUE INDEX entry_one_root
    ON entry(owner_account_id)
    WHERE state = 'active' AND parent_id IS NULL AND library_name IS NULL;
CREATE UNIQUE INDEX entry_library_name
    ON entry(owner_account_id, coalesce(library_name_key, CAST(library_display_name AS BLOB)))
    WHERE state = 'active' AND parent_id IS NULL AND kind = 'folder';

UPDATE installation SET schema_version=4;
PRAGMA user_version=4;
