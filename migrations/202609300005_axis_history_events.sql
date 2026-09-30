ALTER TABLE document_operations
    DROP CONSTRAINT document_operations_principal_matches_actor;
ALTER TABLE document_operations
    ADD CONSTRAINT document_operations_principal_matches_actor
        CHECK (
            (principal_kind = 'user' AND actor_id IS NOT NULL AND schema_version IN (1, 2, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22))
            OR
            (principal_kind = 'system' AND actor_id IS NULL AND schema_version = 3)
        );

ALTER TABLE document_snapshots
    DROP CONSTRAINT document_snapshots_snapshot_schema_version_check;
ALTER TABLE document_snapshots
    ADD CONSTRAINT document_snapshots_snapshot_schema_version_check
    CHECK (snapshot_schema_version IN (1, 2, 3));

ALTER TABLE documents
    ADD COLUMN axis_activation_version bigint NOT NULL DEFAULT 0
        CHECK (axis_activation_version >= 0 AND axis_activation_version <= head_version);
