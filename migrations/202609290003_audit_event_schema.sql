ALTER TABLE audit_events
    ADD COLUMN schema_version integer NOT NULL DEFAULT 1 CHECK (schema_version = 1);
