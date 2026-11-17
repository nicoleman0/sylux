CREATE TABLE removal (
 operation_id TEXT PRIMARY KEY NOT NULL CHECK(length(operation_id)=32 AND operation_id NOT GLOB '*[^0-9a-f]*'),
 owner_account_id TEXT NOT NULL REFERENCES account(account_id),
 confirmation_hash BLOB NOT NULL CHECK(length(confirmation_hash)=32),
 removed_count INTEGER NOT NULL CHECK(removed_count>0),
 committed_at INTEGER NOT NULL CHECK(committed_at>=0)
);
CREATE TABLE removal_member (
 entry_id TEXT PRIMARY KEY NOT NULL REFERENCES entry(entry_id),
 operation_id TEXT NOT NULL REFERENCES removal(operation_id)
);
CREATE TABLE removal_cleanup (
 blob_id TEXT PRIMARY KEY NOT NULL REFERENCES blob(blob_id),
 operation_id TEXT NOT NULL REFERENCES removal(operation_id),
 state TEXT NOT NULL CHECK(state IN('pending','complete'))
);
CREATE INDEX removal_member_operation ON removal_member(operation_id,entry_id);
CREATE INDEX removal_cleanup_operation ON removal_cleanup(operation_id,state);
CREATE INDEX removal_cleanup_pending ON removal_cleanup(state,blob_id);
CREATE TABLE binding_transition (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 legacy_manifest_hash BLOB NOT NULL CHECK(length(legacy_manifest_hash)=32)
);
CREATE TRIGGER removal_immutable_update BEFORE UPDATE ON removal BEGIN SELECT RAISE(ABORT,'removal results are immutable'); END;
CREATE TRIGGER removal_immutable_delete BEFORE DELETE ON removal BEGIN SELECT RAISE(ABORT,'removal results are immutable'); END;
CREATE TRIGGER removal_member_immutable_update BEFORE UPDATE ON removal_member BEGIN SELECT RAISE(ABORT,'removal membership is immutable'); END;
CREATE TRIGGER removal_member_immutable_delete BEFORE DELETE ON removal_member BEGIN SELECT RAISE(ABORT,'removal membership is immutable'); END;
CREATE TRIGGER removal_member_owner BEFORE INSERT ON removal_member BEGIN
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM entry e JOIN removal r ON r.owner_account_id=e.owner_account_id WHERE e.entry_id=NEW.entry_id AND r.operation_id=NEW.operation_id AND e.parent_id IS NOT NULL) THEN RAISE(ABORT,'removal requires same-owner non-root entry') END;
END;
CREATE TRIGGER revision_requires_live_blob BEFORE INSERT ON revision BEGIN
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM blob WHERE blob_id=NEW.blob_id AND lifecycle='live') OR EXISTS(SELECT 1 FROM removal_member WHERE entry_id=NEW.entry_id) THEN RAISE(ABORT,'revision requires retained entry and live blob') END;
END;
CREATE TRIGGER removed_entry_stays_removed BEFORE UPDATE OF state,parent_id,owner_account_id,current_revision_id ON entry WHEN EXISTS(SELECT 1 FROM removal_member WHERE entry_id=OLD.entry_id) BEGIN SELECT RAISE(ABORT,'removed entry is immutable'); END;
CREATE TRIGGER blob_delete_authorization BEFORE UPDATE OF lifecycle ON blob WHEN NEW.lifecycle!='live' BEGIN
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM removal_cleanup WHERE blob_id=OLD.blob_id) OR EXISTS(SELECT 1 FROM revision r WHERE r.blob_id=OLD.blob_id AND NOT EXISTS(SELECT 1 FROM removal_member m WHERE m.entry_id=r.entry_id)) THEN RAISE(ABORT,'blob deletion requires authorization without retained references') END;
END;
CREATE TRIGGER blob_no_resurrection BEFORE UPDATE OF lifecycle ON blob WHEN OLD.lifecycle!='live' AND NEW.lifecycle='live' BEGIN SELECT RAISE(ABORT,'deleted blob cannot become live'); END;
UPDATE installation SET schema_version=2;
PRAGMA user_version=2;
CREATE TRIGGER removed_parent_rejects_insert BEFORE INSERT ON entry WHEN EXISTS(SELECT 1 FROM removal_member WHERE entry_id=NEW.parent_id) BEGIN SELECT RAISE(ABORT,'removed parent cannot receive entries'); END;
CREATE TRIGGER removed_parent_rejects_move BEFORE UPDATE OF parent_id ON entry WHEN EXISTS(SELECT 1 FROM removal_member WHERE entry_id=NEW.parent_id) BEGIN SELECT RAISE(ABORT,'removed parent cannot receive entries'); END;
CREATE TRIGGER removal_cleanup_immutable_delete BEFORE DELETE ON removal_cleanup BEGIN SELECT RAISE(ABORT,'cleanup authorization is immutable'); END;
CREATE TRIGGER removal_cleanup_only_completion BEFORE UPDATE ON removal_cleanup WHEN NEW.blob_id!=OLD.blob_id OR NEW.operation_id!=OLD.operation_id OR (OLD.state='complete' AND NEW.state!='complete') BEGIN SELECT RAISE(ABORT,'cleanup authorization permits completion only'); END;
CREATE TRIGGER removal_cleanup_requires_members BEFORE INSERT ON removal_cleanup BEGIN
 SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM revision r JOIN removal_member m ON m.entry_id=r.entry_id WHERE r.blob_id=NEW.blob_id AND m.operation_id=NEW.operation_id) OR EXISTS(SELECT 1 FROM revision r WHERE r.blob_id=NEW.blob_id AND NOT EXISTS(SELECT 1 FROM removal_member m WHERE m.entry_id=r.entry_id)) THEN RAISE(ABORT,'cleanup requires only removed references') END;
END;
