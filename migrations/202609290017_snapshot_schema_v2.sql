ALTER TABLE document_snapshots
    DROP CONSTRAINT document_snapshots_snapshot_schema_version_check;

ALTER TABLE document_snapshots
    ADD CONSTRAINT document_snapshots_snapshot_schema_version_check
    CHECK (snapshot_schema_version IN (1, 2));
