-- Policy DTO/storage schema 1 is independent of workbook/wire codecs.
CREATE TABLE disclosure_policies (
    scope_id uuid PRIMARY KEY,
    document_id uuid NOT NULL REFERENCES documents(document_id),
    link_id uuid REFERENCES share_links(link_id),
    revision bigint NOT NULL CHECK (revision > 0),
    payload bytea NOT NULL CHECK (octet_length(payload) BETWEEN 57 AND 4152),
    CHECK (scope_id = COALESCE(link_id, document_id))
);
CREATE INDEX disclosure_policies_document ON disclosure_policies(document_id);
CREATE TABLE disclosure_audit_events (
    event_id uuid PRIMARY KEY,
    document_id uuid NOT NULL REFERENCES documents(document_id),
    actor_id uuid NOT NULL REFERENCES users(user_id),
    link_id uuid REFERENCES share_links(link_id),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    document_version bigint NOT NULL CHECK (document_version >= 0),
    book_revision bigint NOT NULL CHECK (book_revision >= 0),
    link_revision bigint NOT NULL CHECK (link_revision >= 0),
    action text NOT NULL CHECK (action IN ('copy','cut','export')),
    format text NOT NULL CHECK (format IN ('clipboard','xlsx')),
    allowed boolean NOT NULL,
    reason text NOT NULL CHECK (reason IN ('allowed','role_denied','copy_denied','format_denied','anonymization_required','policy_changed','version_changed'))
);
CREATE INDEX disclosure_audit_document ON disclosure_audit_events(document_id,event_id DESC);
CREATE FUNCTION reject_disclosure_audit_mutation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'disclosure audit is append-only' USING ERRCODE = '55000';
END
$$;
CREATE TRIGGER disclosure_audit_immutable BEFORE UPDATE OR DELETE ON disclosure_audit_events
    FOR EACH ROW EXECUTE FUNCTION reject_disclosure_audit_mutation();
