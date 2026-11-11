-- Sylux S1 core schema, migration 0001 (schema_version 1).
--
-- Encoding contract (data-model.md / s1-design-review.md §4):
--   * Opaque application IDs are 128-bit server-random values stored as exactly
--     32 lowercase hexadecimal TEXT characters; the product generates them and
--     never accepts user-supplied IDs through this schema.
--   * Integrity digests are raw 32-byte SHA-256 BLOBs (never hex TEXT).
--   * Name sort keys are NFC-normalized UTF-8 bytes stored as BLOB; uniqueness
--     and ordering compare those bytes, never a database collation.
--   * Timestamps are nonnegative INTEGER unix seconds (UTC).
--   * Byte lengths are nonnegative INTEGER.
--
-- Only the S1 core tables exist here (installation, account, session, entry,
-- blob, revision, mutation, upload_attempt, event). No sharing, retention,
-- trash, DAV, GC or future-slice tables. Application-level invariants that
-- SQLite cannot check per row (ancestry depth, NFC normalization itself,
-- manifest agreement) are validated inside the same transaction by the
-- coordinator; the explicit CHECK/FK/UNIQUE/trigger constraints below backstop
-- them.
--
-- The whole migration is one transaction; PRAGMA user_version is transactional
-- and moves with it (verified against SQLite semantics, see evidence/poc/p2).

BEGIN IMMEDIATE;

CREATE TABLE installation (
    singleton        INTEGER PRIMARY KEY CHECK (singleton = 1),
    installation_id  TEXT    NOT NULL CHECK (length(installation_id) = 32
                                          AND installation_id NOT GLOB '*[^0-9a-f]*'),
    root_id          TEXT    NOT NULL CHECK (length(root_id) = 32
                                          AND root_id NOT GLOB '*[^0-9a-f]*'),
    db_id            TEXT    NOT NULL CHECK (length(db_id) = 32
                                          AND db_id NOT GLOB '*[^0-9a-f]*'),
    schema_version   INTEGER NOT NULL CHECK (schema_version > 0),
    layout_version   INTEGER NOT NULL CHECK (layout_version > 0),
    name_key_version INTEGER NOT NULL CHECK (name_key_version > 0),
    key_version      INTEGER NOT NULL CHECK (key_version > 0),
    initialized      INTEGER NOT NULL CHECK (initialized IN (0, 1)),
    recovery_epoch   INTEGER NOT NULL CHECK (recovery_epoch >= 0),
    setup_verifier   BLOB    NOT NULL CHECK (length(setup_verifier) = 32),
    setup_consumed   INTEGER NOT NULL CHECK (setup_consumed IN (0, 1))
);

CREATE TABLE account (
    account_id        TEXT    NOT NULL PRIMARY KEY CHECK (length(account_id) = 32
                                                        AND account_id NOT GLOB '*[^0-9a-f]*'),
    handle            TEXT    NOT NULL UNIQUE
                              -- Two-idiom form: in SQLite GLOB a class
                              -- cannot be repeated with `*` (the star is an
                              -- independent wildcard after the class), so
                              -- the first character gets its own pattern and
                              -- every character is then constrained by the
                              -- negated-class idiom.
                              CHECK (handle GLOB '[a-z0-9]*'
                                     AND handle NOT GLOB '*[^a-z0-9._-]*'
                                     AND length(handle) <= 64),
    role              TEXT    NOT NULL CHECK (role IN ('administrator', 'user')),
    enabled           INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    credential_epoch  INTEGER NOT NULL CHECK (credential_epoch >= 0),
    -- Salted Argon2id PHC string; never a password, hash-of-password shortcut
    -- or normalized/truncated derivative. Null until the owner sets one.
    password_verifier TEXT    CHECK (password_verifier IS NULL
                                     OR password_verifier GLOB '$argon2id$*')
);

CREATE TABLE session (
    session_id        TEXT    NOT NULL PRIMARY KEY CHECK (length(session_id) = 32
                                                        AND session_id NOT GLOB '*[^0-9a-f]*'),
    account_id        TEXT    NOT NULL REFERENCES account(account_id),
    credential_epoch  INTEGER NOT NULL CHECK (credential_epoch >= 0),
    -- SHA-256 of an independent 256-bit random secret; the raw secret is
    -- returned once and never persisted.
    verifier          BLOB    NOT NULL CHECK (length(verifier) = 32),
    csrf_verifier     BLOB    NOT NULL CHECK (length(csrf_verifier) = 32),
    created_at        INTEGER NOT NULL CHECK (created_at >= 0),
    idle_expires_at   INTEGER NOT NULL CHECK (idle_expires_at >= 0),
    absolute_expires_at INTEGER NOT NULL CHECK (absolute_expires_at >= 0),
    revoked           INTEGER NOT NULL DEFAULT 0 CHECK (revoked IN (0, 1))
);
CREATE INDEX session_account ON session(account_id);

CREATE TABLE blob (
    blob_id   TEXT    NOT NULL PRIMARY KEY CHECK (length(blob_id) = 32
                                                AND blob_id NOT GLOB '*[^0-9a-f]*'),
    -- Shard is derived from the blob ID (first two hex characters); keeping the
    -- derivation checked here prevents accidental cross-shard placement.
    shard     TEXT    NOT NULL CHECK (shard = substr(blob_id, 1, 2)),
    length    INTEGER NOT NULL CHECK (length >= 0),
    sha256    BLOB    NOT NULL CHECK (length(sha256) = 32),
    lifecycle TEXT    NOT NULL DEFAULT 'live' CHECK (lifecycle IN ('live', 'deleting', 'deleted'))
);

CREATE TABLE revision (
    revision_id  TEXT    NOT NULL PRIMARY KEY CHECK (length(revision_id) = 32
                                                   AND revision_id NOT GLOB '*[^0-9a-f]*'),
    entry_id     TEXT    NOT NULL REFERENCES entry(entry_id),
    blob_id      TEXT    NOT NULL REFERENCES blob(blob_id),
    length       INTEGER NOT NULL CHECK (length >= 0),
    sha256       BLOB    NOT NULL CHECK (length(sha256) = 32),
    committed_at INTEGER NOT NULL CHECK (committed_at >= 0),
    actor_account_id TEXT NOT NULL REFERENCES account(account_id)
);
CREATE INDEX revision_entry_time ON revision(entry_id, committed_at, revision_id);
CREATE INDEX revision_blob ON revision(blob_id);

CREATE TABLE entry (
    entry_id        TEXT    NOT NULL PRIMARY KEY CHECK (length(entry_id) = 32
                                                      AND entry_id NOT GLOB '*[^0-9a-f]*'),
    owner_account_id TEXT   NOT NULL REFERENCES account(account_id),
    -- Null parent is allowed only for the single account root; roots are
    -- immutable and named by the empty byte string.
    parent_id       TEXT    REFERENCES entry(entry_id),
    kind            TEXT    NOT NULL CHECK (kind IN ('file', 'folder')),
    original_name   TEXT    NOT NULL,
    name_key        BLOB    NOT NULL,
    name_key_version INTEGER NOT NULL CHECK (name_key_version > 0),
    -- Null for folders, and transiently null for files inside the single
    -- transaction that creates the entry together with its first revision.
    current_revision_id TEXT REFERENCES revision(revision_id),
    state           TEXT    NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'deleted')),
    generation      INTEGER NOT NULL DEFAULT 1 CHECK (generation >= 1),
    CHECK (kind != 'folder' OR current_revision_id IS NULL),
    CHECK ((parent_id IS NULL
            AND length(CAST(original_name AS BLOB)) = 0
            AND length(name_key) = 0)
        OR (parent_id IS NOT NULL
            AND length(CAST(original_name AS BLOB)) BETWEEN 1 AND 255
            AND length(name_key) BETWEEN 1 AND 255))
);

-- Parent must be an active same-owner folder; enforced on insert and on any
-- parent/owner change. Root rows (null parent) are excluded.
CREATE TRIGGER entry_insert_parent_active_folder
BEFORE INSERT ON entry
BEGIN
    SELECT CASE WHEN NEW.parent_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM entry parent
        WHERE parent.entry_id = NEW.parent_id
          AND parent.owner_account_id = NEW.owner_account_id
          AND parent.kind = 'folder'
          AND parent.state = 'active'
    ) THEN RAISE(ABORT, 'entry parent must be an active same-owner folder') END;
END;

CREATE TRIGGER entry_update_parent_active_folder
BEFORE UPDATE OF parent_id, owner_account_id ON entry
BEGIN
    SELECT CASE WHEN NEW.parent_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM entry parent
        WHERE parent.entry_id = NEW.parent_id
          AND parent.owner_account_id = NEW.owner_account_id
          AND parent.kind = 'folder'
          AND parent.state = 'active'
    ) THEN RAISE(ABORT, 'entry parent must be an active same-owner folder') END;
END;

-- No ancestry cycles. A freshly inserted row has no children, so only a
-- self-parent can cycle on INSERT; re-parenting walks the candidate parent's
-- ancestry and refuses if the moved entry would become its own ancestor.
-- UNION (not UNION ALL) keeps the walk terminating even on damaged data.
CREATE TRIGGER entry_insert_no_parent_cycle
BEFORE INSERT ON entry
BEGIN
    SELECT CASE WHEN NEW.parent_id IS NOT NULL AND EXISTS (
        WITH RECURSIVE ancestors(id) AS (
            SELECT NEW.parent_id
            UNION
            SELECT parent.parent_id FROM entry parent
            JOIN ancestors ON parent.entry_id = ancestors.id
        )
        SELECT 1 FROM ancestors WHERE id = NEW.entry_id
    ) THEN RAISE(ABORT, 'entry parent would create a cycle') END;
END;

CREATE TRIGGER entry_update_no_parent_cycle
BEFORE UPDATE OF parent_id ON entry
BEGIN
    SELECT CASE WHEN NEW.parent_id IS NOT NULL
                    AND NEW.parent_id IS NOT OLD.parent_id
                    AND EXISTS (
        WITH RECURSIVE ancestors(id) AS (
            SELECT NEW.parent_id
            UNION
            SELECT parent.parent_id FROM entry parent
            JOIN ancestors ON parent.entry_id = ancestors.id
        )
        SELECT 1 FROM ancestors WHERE id = NEW.entry_id
    ) THEN RAISE(ABORT, 'entry parent would create a cycle') END;
END;

-- The current pointer must reference a revision of this very entry.
CREATE TRIGGER entry_insert_current_same_entry
BEFORE INSERT ON entry
BEGIN
    SELECT CASE WHEN NEW.current_revision_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM revision
        WHERE revision.revision_id = NEW.current_revision_id
          AND revision.entry_id = NEW.entry_id
    ) THEN RAISE(ABORT, 'entry current revision must belong to the same entry') END;
END;

CREATE TRIGGER entry_update_current_same_entry
BEFORE UPDATE OF current_revision_id ON entry
BEGIN
    SELECT CASE WHEN NEW.current_revision_id IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM revision
        WHERE revision.revision_id = NEW.current_revision_id
          AND revision.entry_id = NEW.entry_id
    ) THEN RAISE(ABORT, 'entry current revision must belong to the same entry') END;
END;

-- Revisions are the immutable seam (FR-006): no UPDATE, no DELETE. Retention
-- and restore flows in later slices must extend this schema through a reviewed
-- migration, never by rewriting committed revisions.
CREATE TRIGGER revision_rows_are_immutable
BEFORE UPDATE ON revision
BEGIN
    SELECT RAISE(ABORT, 'revision rows are immutable');
END;

CREATE TRIGGER revision_rows_cannot_be_deleted
BEFORE DELETE ON revision
BEGIN
    SELECT RAISE(ABORT, 'revision rows cannot be deleted');
END;

-- Active sibling names are unique per owner and parent across both kinds;
-- root uniqueness is enforced separately because nullable SQL unique keys
-- do not compare NULL parents.
CREATE UNIQUE INDEX entry_active_sibling
    ON entry(owner_account_id, parent_id, name_key)
    WHERE state = 'active' AND parent_id IS NOT NULL;

CREATE UNIQUE INDEX entry_one_root
    ON entry(owner_account_id)
    WHERE state = 'active' AND parent_id IS NULL;

CREATE INDEX entry_active_listing
    ON entry(owner_account_id, parent_id, state, name_key, entry_id);

CREATE TABLE mutation (
    mutation_id          TEXT    NOT NULL PRIMARY KEY CHECK (length(mutation_id) = 32
                                                          AND mutation_id NOT GLOB '*[^0-9a-f]*'),
    principal_account_id TEXT    NOT NULL REFERENCES account(account_id),
    scope                TEXT    NOT NULL CHECK (length(scope) BETWEEN 1 AND 64),
    ticket_key_id        TEXT    NOT NULL CHECK (length(ticket_key_id) BETWEEN 1 AND 64),
    ticket_epoch         INTEGER NOT NULL CHECK (ticket_epoch >= 0),
    issued_at            INTEGER NOT NULL CHECK (issued_at >= 0),
    expires_at           INTEGER NOT NULL CHECK (expires_at >= issued_at),
    -- SHA-256 of the canonical versioned admission encoding.
    admission_fingerprint BLOB   NOT NULL CHECK (length(admission_fingerprint) = 32),
    -- Fixed only after the completed body is durably synced; null until then.
    completed_sha256     BLOB    CHECK (completed_sha256 IS NULL
                                        OR length(completed_sha256) = 32),
    completed_length     INTEGER CHECK (completed_length IS NULL
                                        OR completed_length >= 0),
    state                TEXT    NOT NULL CHECK (state IN ('admitted', 'publishing',
                                                           'committed', 'rejected')),
    result_revision_id   TEXT    REFERENCES revision(revision_id),
    UNIQUE (principal_account_id, mutation_id)
);
CREATE INDEX mutation_principal ON mutation(principal_account_id, mutation_id);

CREATE TABLE upload_attempt (
    attempt_id       TEXT    NOT NULL PRIMARY KEY CHECK (length(attempt_id) = 32
                                                       AND attempt_id NOT GLOB '*[^0-9a-f]*'),
    mutation_id      TEXT    NOT NULL REFERENCES mutation(mutation_id),
    attempt_no       INTEGER NOT NULL CHECK (attempt_no >= 1),
    staging_path     TEXT    NOT NULL CHECK (length(staging_path) BETWEEN 1 AND 255
                                             AND staging_path NOT GLOB '*\*'),
    writer_active    INTEGER NOT NULL DEFAULT 1 CHECK (writer_active IN (0, 1)),
    bytes_received   INTEGER NOT NULL DEFAULT 0 CHECK (bytes_received >= 0),
    completed_length INTEGER CHECK (completed_length IS NULL OR completed_length >= 0),
    completed_sha256 BLOB    CHECK (completed_sha256 IS NULL
                                    OR length(completed_sha256) = 32),
    outcome          TEXT    CHECK (outcome IS NULL OR outcome IN ('canceled', 'rejected')),
    UNIQUE (mutation_id, attempt_no)
);
-- At most one active body stream per admitted operation.
CREATE UNIQUE INDEX upload_attempt_one_active
    ON upload_attempt(mutation_id)
    WHERE writer_active = 1;

CREATE TABLE event (
    event_id         INTEGER PRIMARY KEY,
    actor_account_id TEXT    REFERENCES account(account_id),
    action           TEXT    NOT NULL CHECK (length(action) BETWEEN 1 AND 128),
    occurred_at      INTEGER NOT NULL CHECK (occurred_at >= 0),
    outcome          TEXT    NOT NULL CHECK (outcome IN ('ok', 'denied', 'error'))
);

PRAGMA user_version = 1;

COMMIT;
