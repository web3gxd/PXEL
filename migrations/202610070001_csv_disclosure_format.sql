-- Disclosure records have an independent codec lifecycle; old ciphertext stays v1.
ALTER TABLE disclosure_policies ADD COLUMN codec_version integer NOT NULL DEFAULT 1
    CHECK (codec_version IN (1,2));
ALTER TABLE disclosure_audit_events DROP CONSTRAINT disclosure_audit_events_format_check;
ALTER TABLE disclosure_audit_events ADD CONSTRAINT disclosure_audit_events_format_check
    CHECK (format IN ('clipboard','xlsx','pxel','csv'));
