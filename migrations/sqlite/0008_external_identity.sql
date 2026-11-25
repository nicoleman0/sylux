ALTER TABLE account ADD COLUMN credential_mode TEXT NOT NULL DEFAULT 'local'
    CHECK (credential_mode = 'local'
        OR (credential_mode = 'external' AND role = 'user' AND password_verifier IS NULL));

CREATE TABLE external_provider (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    registration BLOB NOT NULL CHECK (length(registration) = 32)
) STRICT;

CREATE TABLE external_identity (
    identity_id TEXT PRIMARY KEY NOT NULL CHECK (length(identity_id) = 32),
    issuer TEXT NOT NULL COLLATE BINARY CHECK (length(issuer) BETWEEN 1 AND 2048),
    subject TEXT NOT NULL COLLATE BINARY CHECK (length(subject) BETWEEN 1 AND 255),
    account_id TEXT REFERENCES account(account_id),
    handle TEXT NOT NULL COLLATE BINARY,
    state TEXT NOT NULL CHECK (state IN ('pending','active','revoked')),
    generation INTEGER NOT NULL CHECK (generation >= 0),
    approved_by TEXT NOT NULL REFERENCES account(account_id),
    approver_epoch INTEGER NOT NULL CHECK (approver_epoch >= 0),
    target_epoch INTEGER CHECK (target_epoch >= 0),
    CHECK ((account_id IS NULL) = (target_epoch IS NULL)),
    CHECK (state <> 'active' OR account_id IS NOT NULL),
    UNIQUE (issuer, subject)
) STRICT;

CREATE UNIQUE INDEX external_identity_current_account ON external_identity(account_id)
    WHERE state IN ('pending','active') AND account_id IS NOT NULL;
CREATE UNIQUE INDEX external_identity_pending_handle ON external_identity(handle)
    WHERE state = 'pending';

ALTER TABLE session ADD COLUMN external_identity_id TEXT REFERENCES external_identity(identity_id);
ALTER TABLE session ADD COLUMN external_generation INTEGER
    CHECK ((external_identity_id IS NULL AND external_generation IS NULL)
        OR (external_identity_id IS NOT NULL AND external_generation IS NOT NULL AND external_generation >= 0));
ALTER TABLE event ADD COLUMN target_account_id TEXT REFERENCES account(account_id);
ALTER TABLE event ADD COLUMN external_identity_id TEXT REFERENCES external_identity(identity_id);

UPDATE installation SET schema_version=8;
PRAGMA user_version=8;
