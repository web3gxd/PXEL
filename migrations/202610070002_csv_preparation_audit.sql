-- Preparation failures are distinct from privacy analysis failures.
ALTER TABLE disclosure_audit_events DROP CONSTRAINT disclosure_audit_events_reason_check;
ALTER TABLE disclosure_audit_events ADD CONSTRAINT disclosure_audit_events_reason_check
    CHECK (reason IN ('allowed','role_denied','copy_denied','format_denied',
        'anonymization_required','policy_changed','version_changed',
        'privacy_analysis_failed','preparation_failed'));
