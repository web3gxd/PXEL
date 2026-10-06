ALTER TABLE document_operations
    DROP CONSTRAINT document_operations_principal_matches_actor;
ALTER TABLE document_operations
    ADD CONSTRAINT document_operations_principal_matches_actor CHECK (
        (principal_kind = 'user' AND actor_id IS NOT NULL AND schema_version IN
            (1, 2, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17,
             18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
             32, 33, 34, 35, 36, 37, 38))
        OR
        (principal_kind = 'system' AND actor_id IS NULL AND schema_version = 3)
    );
