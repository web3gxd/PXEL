-- ACL mutations have their own durable control history, separate from document content events.
CREATE TABLE audit_events (
    event_id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces(workspace_id),
    document_id uuid NOT NULL REFERENCES documents(document_id),
    actor_id uuid NOT NULL REFERENCES users(user_id),
    target_user_id uuid NOT NULL REFERENCES users(user_id),
    event_type text NOT NULL CHECK (event_type IN ('permission_granted', 'permission_revoked')),
    old_role text CHECK (old_role IN ('owner', 'editor', 'viewer')),
    new_role text CHECK (new_role IN ('editor', 'viewer')),
    access_epoch bigint NOT NULL CHECK (access_epoch > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (document_id, access_epoch),
    FOREIGN KEY (workspace_id, document_id) REFERENCES documents(workspace_id, document_id)
);
CREATE INDEX audit_events_document_created_idx ON audit_events(document_id, created_at);
CREATE FUNCTION reject_audit_mutation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'audit events are append only' USING ERRCODE='55000'; END $$;
CREATE TRIGGER audit_events_immutable BEFORE UPDATE OR DELETE ON audit_events
 FOR EACH ROW EXECUTE FUNCTION reject_audit_mutation();
