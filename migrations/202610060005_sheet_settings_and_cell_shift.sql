ALTER TABLE document_operations DROP CONSTRAINT document_operations_principal_matches_actor;
ALTER TABLE document_operations ADD CONSTRAINT document_operations_principal_matches_actor CHECK (
 (principal_kind='user' AND actor_id IS NOT NULL AND schema_version BETWEEN 1 AND 42 AND schema_version <> 3)
 OR (principal_kind='system' AND actor_id IS NULL AND schema_version=3));
ALTER TABLE document_snapshots DROP CONSTRAINT document_snapshots_snapshot_schema_version_check;
ALTER TABLE document_snapshots ADD CONSTRAINT document_snapshots_snapshot_schema_version_check CHECK(snapshot_schema_version BETWEEN 1 AND 8);
