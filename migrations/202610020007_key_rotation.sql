CREATE TABLE crypto_key_state (
    singleton boolean PRIMARY KEY CHECK (singleton),
    active_kek_id text NOT NULL CHECK (length(active_kek_id) BETWEEN 1 AND 64),
    rotated_at timestamptz
);
CREATE FUNCTION guard_document_workspace_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.workspace_id IS DISTINCT FROM OLD.workspace_id OR NEW.document_id IS DISTINCT FROM OLD.document_id THEN
        RAISE EXCEPTION 'document workspace identity is immutable' USING ERRCODE='55000';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER documents_workspace_identity
    BEFORE UPDATE ON documents FOR EACH ROW EXECUTE FUNCTION guard_document_workspace_identity();
