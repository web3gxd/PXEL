-- Schema 10 atomically persists a bounded set of distinct cell value writes.
ALTER TABLE document_operations
    DROP CONSTRAINT document_operations_principal_matches_actor;
ALTER TABLE document_operations
    ADD CONSTRAINT document_operations_principal_matches_actor
        CHECK (
            (principal_kind = 'user' AND actor_id IS NOT NULL AND schema_version IN (1, 2, 4, 5, 6, 7, 8, 9, 10))
            OR
            (principal_kind = 'system' AND actor_id IS NULL AND schema_version = 3)
        );
