-- Separate append-only export preparation audit. No document content is stored here.
CREATE TABLE export_audit_events (
    event_id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    document_id uuid NOT NULL,
    actor_id uuid NOT NULL REFERENCES users(user_id),
    document_version bigint NOT NULL CHECK (document_version > 0),
    payload_checksum bytea NOT NULL CHECK (octet_length(payload_checksum) = 32),
    payload_size bigint NOT NULL CHECK (payload_size > 0 AND payload_size <= 33554432),
    created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (workspace_id, document_id) REFERENCES documents(workspace_id, document_id)
);
CREATE INDEX export_audit_document_created_idx ON export_audit_events(document_id, created_at);
CREATE TRIGGER export_audit_events_immutable BEFORE UPDATE OR DELETE ON export_audit_events
 FOR EACH ROW EXECUTE FUNCTION reject_audit_mutation();
