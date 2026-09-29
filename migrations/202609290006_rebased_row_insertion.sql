-- Schema 6 preserves schema-5 insertion semantics while recording a distinct
-- deterministic formula-offset transform for new row insertions.
ALTER TABLE document_operations
    DROP CONSTRAINT document_operations_principal_matches_actor;
ALTER TABLE document_operations
    ADD CONSTRAINT document_operations_principal_matches_actor
        CHECK (
            (principal_kind = 'user' AND actor_id IS NOT NULL AND schema_version IN (1, 2, 4, 5, 6))
            OR
            (principal_kind = 'system' AND actor_id IS NULL AND schema_version = 3)
        );
