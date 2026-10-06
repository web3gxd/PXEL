ALTER TABLE document_operations DROP CONSTRAINT document_operations_principal_matches_actor;
ALTER TABLE document_operations ADD CONSTRAINT document_operations_principal_matches_actor CHECK (
 (principal_kind='user' AND actor_id IS NOT NULL AND schema_version BETWEEN 1 AND 44 AND schema_version <> 3)
 OR (principal_kind='system' AND actor_id IS NULL AND schema_version=3));
