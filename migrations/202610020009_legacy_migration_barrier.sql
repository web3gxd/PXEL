CREATE TABLE storage_migration_state (
    singleton boolean PRIMARY KEY CHECK (singleton),
    state text NOT NULL CHECK (state IN ('PENDING', 'COMPLETE')),
    started_at timestamptz NOT NULL DEFAULT now(),
    completed_at timestamptz,
    CHECK ((state = 'PENDING' AND completed_at IS NULL)
        OR (state = 'COMPLETE' AND completed_at IS NOT NULL))
);
CREATE FUNCTION guard_storage_migration_state() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'migration progress is retained' USING ERRCODE = '55000';
    END IF;
    IF NEW.singleton IS DISTINCT FROM OLD.singleton
       OR NEW.started_at IS DISTINCT FROM OLD.started_at
       OR OLD.state <> 'PENDING' OR NEW.state <> 'COMPLETE' THEN
        RAISE EXCEPTION 'invalid migration activation' USING ERRCODE = '55000';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER storage_migration_state_guard BEFORE UPDATE OR DELETE ON storage_migration_state
    FOR EACH ROW EXECUTE FUNCTION guard_storage_migration_state();
CREATE TRIGGER storage_migration_state_no_truncate BEFORE TRUNCATE ON storage_migration_state
    FOR EACH STATEMENT EXECUTE FUNCTION reject_journal_mutation();
