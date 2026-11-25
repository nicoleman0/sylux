CREATE TABLE dav_credential (
    credential_id TEXT PRIMARY KEY CHECK (length(credential_id) = 32),
    account_id TEXT NOT NULL REFERENCES account(account_id),
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 80),
    verifier BLOB NOT NULL CHECK (length(verifier) = 32),
    credential_epoch INTEGER NOT NULL CHECK (credential_epoch >= 0),
    read_only INTEGER NOT NULL CHECK (read_only IN (0, 1)),
    UNIQUE (account_id, name)
) STRICT;

UPDATE installation SET schema_version=5;
PRAGMA user_version=5;
