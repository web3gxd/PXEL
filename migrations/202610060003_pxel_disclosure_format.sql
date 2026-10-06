-- File format lifecycle is independent of the journal/snapshot families.
ALTER TABLE disclosure_audit_events DROP CONSTRAINT disclosure_audit_events_format_check;
ALTER TABLE disclosure_audit_events ADD CONSTRAINT disclosure_audit_events_format_check CHECK (format IN ('clipboard','xlsx','pxel'));
