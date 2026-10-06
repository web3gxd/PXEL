CREATE TABLE document_privacy_policies (
    document_id uuid PRIMARY KEY REFERENCES documents(document_id),
    revision bigint NOT NULL CHECK (revision > 0),
    payload bytea NOT NULL CHECK (is_encrypted_storage_envelope(payload) AND octet_length(payload) <= 131128)
);
ALTER TABLE share_links ADD COLUMN anonymized_at_creation boolean NOT NULL DEFAULT false;
ALTER TABLE disclosure_audit_events DROP CONSTRAINT disclosure_audit_events_reason_check;
ALTER TABLE disclosure_audit_events ADD CONSTRAINT disclosure_audit_events_reason_check CHECK (reason IN ('allowed','role_denied','copy_denied','format_denied','anonymization_required','policy_changed','version_changed','privacy_analysis_failed'));
