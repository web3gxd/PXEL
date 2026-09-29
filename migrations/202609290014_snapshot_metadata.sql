CREATE TABLE document_snapshots (
    snapshot_id uuid PRIMARY KEY,
    document_id uuid NOT NULL REFERENCES documents(document_id),
    snapshot_version bigint NOT NULL CHECK (snapshot_version > 0),
    last_axis_change_version bigint NOT NULL CHECK (
        last_axis_change_version > 0 AND last_axis_change_version <= snapshot_version
    ),
    snapshot_schema_version integer NOT NULL CHECK (snapshot_schema_version = 1),
    object_key uuid NOT NULL UNIQUE,
    byte_length bigint NOT NULL CHECK (byte_length > 0 AND byte_length <= 33554432),
    payload_checksum bytea NOT NULL CHECK (octet_length(payload_checksum) = 32),
    state_hash bytea NOT NULL CHECK (octet_length(state_hash) = 32),
    writer_engine_version text NOT NULL CHECK (
        length(writer_engine_version) BETWEEN 1 AND 64
    ),
    writer_compatibility_profile text NOT NULL CHECK (
        length(writer_compatibility_profile) BETWEEN 1 AND 64
    ),
    state text NOT NULL DEFAULT 'UPLOADING' CHECK (
        state IN ('UPLOADING', 'STORED', 'VERIFIED', 'ACTIVE', 'REJECTED')
    ),
    created_at timestamptz NOT NULL DEFAULT now(),
    verified_at timestamptz,
    activated_at timestamptz,
    UNIQUE (document_id, snapshot_id)
);

CREATE INDEX document_snapshots_active_by_version
    ON document_snapshots(document_id, snapshot_version DESC, created_at DESC)
    WHERE state = 'ACTIVE';

ALTER TABLE documents ADD COLUMN preferred_snapshot_id uuid;
ALTER TABLE documents ADD CONSTRAINT documents_preferred_snapshot_same_document
    FOREIGN KEY (document_id, preferred_snapshot_id)
    REFERENCES document_snapshots(document_id, snapshot_id);

CREATE FUNCTION guard_snapshot_state() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.snapshot_id IS DISTINCT FROM OLD.snapshot_id
       OR NEW.document_id IS DISTINCT FROM OLD.document_id
       OR NEW.snapshot_version IS DISTINCT FROM OLD.snapshot_version
       OR NEW.last_axis_change_version IS DISTINCT FROM OLD.last_axis_change_version
       OR NEW.snapshot_schema_version IS DISTINCT FROM OLD.snapshot_schema_version
       OR NEW.object_key IS DISTINCT FROM OLD.object_key
       OR NEW.byte_length IS DISTINCT FROM OLD.byte_length
       OR NEW.payload_checksum IS DISTINCT FROM OLD.payload_checksum
       OR NEW.state_hash IS DISTINCT FROM OLD.state_hash
       OR NEW.writer_engine_version IS DISTINCT FROM OLD.writer_engine_version
       OR NEW.writer_compatibility_profile IS DISTINCT FROM OLD.writer_compatibility_profile
       OR NEW.created_at IS DISTINCT FROM OLD.created_at
    THEN
        RAISE EXCEPTION 'snapshot identity and payload metadata are immutable' USING ERRCODE = '55000';
    END IF;
    IF NOT (
        (OLD.state = 'UPLOADING' AND NEW.state IN ('STORED', 'REJECTED')) OR
        (OLD.state = 'STORED' AND NEW.state IN ('VERIFIED', 'REJECTED')) OR
        (OLD.state = 'VERIFIED' AND NEW.state IN ('ACTIVE', 'REJECTED')) OR
        (OLD.state = 'ACTIVE' AND NEW.state = 'REJECTED')
    ) THEN
        RAISE EXCEPTION 'invalid snapshot state transition' USING ERRCODE = '55000';
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER document_snapshots_state_guard
    BEFORE UPDATE ON document_snapshots
    FOR EACH ROW EXECUTE FUNCTION guard_snapshot_state();
