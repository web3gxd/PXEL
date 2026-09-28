-- Historical schema 1/2 events remain user events. Schema 3 is reserved for
-- internally produced calculation epoch events with no fabricated user actor.
ALTER TABLE document_operations
    ALTER COLUMN actor_id DROP NOT NULL;
ALTER TABLE document_operations
    ADD COLUMN principal_kind text NOT NULL DEFAULT 'user'
        CHECK (principal_kind IN ('user', 'system'));
ALTER TABLE document_operations
    ADD CONSTRAINT document_operations_principal_matches_actor
        CHECK (
            (principal_kind = 'user' AND actor_id IS NOT NULL AND schema_version IN (1, 2))
            OR
            (principal_kind = 'system' AND actor_id IS NULL AND schema_version = 3)
        );
