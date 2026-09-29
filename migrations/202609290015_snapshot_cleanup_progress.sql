ALTER TABLE document_snapshots
    ADD COLUMN cleanup_attempted_at timestamptz;

CREATE INDEX document_snapshots_cleanup_candidates
    ON document_snapshots (cleanup_attempted_at NULLS FIRST, created_at, snapshot_id)
    WHERE state = 'REJECTED';

CREATE OR REPLACE FUNCTION guard_snapshot_state() RETURNS trigger LANGUAGE plpgsql AS $$
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
    IF NEW.cleanup_attempted_at IS DISTINCT FROM OLD.cleanup_attempted_at
       AND NOT (
           OLD.state = 'REJECTED' AND NEW.state = 'REJECTED'
           AND NEW.cleanup_attempted_at IS NOT NULL
           AND (OLD.cleanup_attempted_at IS NULL OR NEW.cleanup_attempted_at >= OLD.cleanup_attempted_at)
       )
    THEN
        RAISE EXCEPTION 'invalid snapshot cleanup progress' USING ERRCODE = '55000';
    END IF;
    IF NOT (
        (OLD.state = 'UPLOADING' AND NEW.state IN ('STORED', 'REJECTED')) OR
        (OLD.state = 'STORED' AND NEW.state IN ('VERIFIED', 'REJECTED')) OR
        (OLD.state = 'VERIFIED' AND NEW.state IN ('ACTIVE', 'REJECTED')) OR
        (OLD.state = 'ACTIVE' AND NEW.state = 'REJECTED') OR
        (OLD.state = 'REJECTED' AND NEW.state = 'REJECTED'
            AND NEW.cleanup_attempted_at IS DISTINCT FROM OLD.cleanup_attempted_at)
    ) THEN
        RAISE EXCEPTION 'invalid snapshot state transition' USING ERRCODE = '55000';
    END IF;
    RETURN NEW;
END $$;
