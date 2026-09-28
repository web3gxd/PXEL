CREATE TABLE document_operations (
 document_id uuid NOT NULL REFERENCES documents(document_id),
 committed_version bigint NOT NULL CHECK(committed_version > 0),
 operation_id uuid NOT NULL,
 actor_id uuid NOT NULL REFERENCES users(user_id),
 schema_version integer NOT NULL CHECK(schema_version > 0),
 payload bytea NOT NULL CHECK(octet_length(payload) <= 4194304),
 payload_checksum bytea NOT NULL CHECK(octet_length(payload_checksum)=32),
 command_fingerprint bytea NOT NULL CHECK(octet_length(command_fingerprint)=32),
 canonical_event_hash bytea NOT NULL CHECK(octet_length(canonical_event_hash)=32),
 created_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY(document_id, committed_version),
 UNIQUE(document_id, operation_id)
);
CREATE FUNCTION reject_journal_mutation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'operation journal is append only' USING ERRCODE='55000'; END $$;
CREATE TRIGGER document_operations_immutable BEFORE UPDATE OR DELETE ON document_operations
 FOR EACH ROW EXECUTE FUNCTION reject_journal_mutation();
CREATE TABLE development_sessions (
 token_hash bytea PRIMARY KEY CHECK(octet_length(token_hash)=32),
 user_id uuid NOT NULL REFERENCES users(user_id),
 workspace_id uuid NOT NULL REFERENCES workspaces(workspace_id),
 expires_at timestamptz NOT NULL DEFAULT now()+interval '1 hour',
 FOREIGN KEY(workspace_id,user_id) REFERENCES memberships(workspace_id,user_id)
);
CREATE INDEX development_sessions_expiry ON development_sessions(expires_at);
