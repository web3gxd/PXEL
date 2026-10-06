CREATE TABLE document_comment_heads (
    document_id uuid PRIMARY KEY REFERENCES documents(document_id),
    revision bigint NOT NULL DEFAULT 0 CHECK(revision >= 0)
);
CREATE TABLE document_comment_events (
    document_id uuid NOT NULL REFERENCES documents(document_id),
    revision bigint NOT NULL CHECK(revision > 0),
    event_id uuid NOT NULL,
    request_id uuid NOT NULL,
    comment_id uuid NOT NULL,
    event_kind text NOT NULL CHECK(event_kind IN ('add','delete')),
    sheet_id uuid NOT NULL,
    row_id uuid NOT NULL,
    column_id uuid NOT NULL,
    actor_id uuid NOT NULL REFERENCES users(user_id),
    payload bytea NOT NULL CHECK(is_encrypted_storage_envelope(payload) AND octet_length(payload) BETWEEN 57 AND 20536),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY(document_id, revision),
    UNIQUE(document_id,event_id),
    UNIQUE(document_id,actor_id,request_id)
);
CREATE INDEX document_comment_current ON document_comment_events(document_id,sheet_id,row_id,column_id,comment_id,revision DESC);
CREATE TRIGGER document_comment_events_immutable BEFORE UPDATE OR DELETE ON document_comment_events
    FOR EACH ROW EXECUTE FUNCTION reject_journal_mutation();
CREATE TRIGGER document_comment_events_no_truncate BEFORE TRUNCATE ON document_comment_events
    FOR EACH STATEMENT EXECUTE FUNCTION reject_journal_mutation();
CREATE FUNCTION guard_document_comment_head() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' OR (TG_OP='UPDATE' AND NEW.document_id IS DISTINCT FROM OLD.document_id)
       OR (TG_OP='UPDATE' AND NEW.revision <> OLD.revision + 1)
       OR (TG_OP='INSERT' AND NEW.revision <> 1) THEN
        RAISE EXCEPTION 'comment revision is append-only' USING ERRCODE='55000';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER document_comment_heads_guard BEFORE INSERT OR UPDATE OR DELETE ON document_comment_heads
    FOR EACH ROW EXECUTE FUNCTION guard_document_comment_head();
CREATE TRIGGER document_comment_heads_no_truncate BEFORE TRUNCATE ON document_comment_heads
    FOR EACH STATEMENT EXECUTE FUNCTION reject_journal_mutation();
