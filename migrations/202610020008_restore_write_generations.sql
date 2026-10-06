-- A copied cluster cannot infer its physical lineage. The unpublished restore
-- tool initializes this durable barrier before replacing any write generation.
CREATE TABLE crypto_restore_runs (
    run_id uuid PRIMARY KEY,
    target_kek_id text NOT NULL CHECK (length(target_kek_id) BETWEEN 1 AND 64),
    target_physical_key_id bytea NOT NULL UNIQUE CHECK (octet_length(target_physical_key_id) = 32),
    state text NOT NULL DEFAULT 'PENDING' CHECK (state IN ('PENDING', 'COMPLETE')),
    created_at timestamptz NOT NULL DEFAULT now(),
    completed_at timestamptz,
    CHECK ((state = 'PENDING' AND completed_at IS NULL)
        OR (state = 'COMPLETE' AND completed_at IS NOT NULL))
);
CREATE TABLE crypto_restore_state (
    singleton boolean PRIMARY KEY CHECK (singleton),
    current_run_id uuid NOT NULL REFERENCES crypto_restore_runs(run_id)
);
CREATE TABLE crypto_restore_documents (
    run_id uuid NOT NULL REFERENCES crypto_restore_runs(run_id),
    document_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    source_max_generation integer NOT NULL CHECK (source_max_generation >= 0 AND source_max_generation < 2147483647),
    new_generation integer NOT NULL CHECK (new_generation = source_max_generation + 1),
    complete boolean NOT NULL DEFAULT false,
    completed_at timestamptz,
    PRIMARY KEY (run_id, document_id),
    UNIQUE (document_id, new_generation),
    CHECK ((NOT complete AND completed_at IS NULL)
        OR (complete AND completed_at IS NOT NULL))
);

CREATE FUNCTION guard_restore_progress() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'restore lineage progress is retained' USING ERRCODE = '55000';
    END IF;
    IF TG_TABLE_NAME = 'crypto_restore_runs' THEN
        IF NEW.run_id IS DISTINCT FROM OLD.run_id
           OR NEW.target_kek_id IS DISTINCT FROM OLD.target_kek_id
           OR NEW.target_physical_key_id IS DISTINCT FROM OLD.target_physical_key_id
           OR NEW.created_at IS DISTINCT FROM OLD.created_at
           OR OLD.state <> 'PENDING' OR NEW.state <> 'COMPLETE'
        THEN
            RAISE EXCEPTION 'invalid restore lineage transition' USING ERRCODE = '55000';
        END IF;
    ELSIF TG_TABLE_NAME = 'crypto_restore_documents' THEN
        IF NEW.run_id IS DISTINCT FROM OLD.run_id
           OR NEW.document_id IS DISTINCT FROM OLD.document_id
           OR NEW.workspace_id IS DISTINCT FROM OLD.workspace_id
           OR NEW.source_max_generation IS DISTINCT FROM OLD.source_max_generation
           OR NEW.new_generation IS DISTINCT FROM OLD.new_generation
           OR OLD.complete OR NOT NEW.complete
        THEN
            RAISE EXCEPTION 'invalid document restore progress' USING ERRCODE = '55000';
        END IF;
    ELSIF TG_TABLE_NAME = 'crypto_restore_state' THEN
        IF NEW.singleton IS DISTINCT FROM OLD.singleton
           OR EXISTS(SELECT 1 FROM crypto_restore_runs WHERE run_id=OLD.current_run_id AND state='PENDING')
        THEN
            RAISE EXCEPTION 'pending restore lineage cannot be replaced' USING ERRCODE = '55000';
        END IF;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER crypto_restore_runs_guard BEFORE UPDATE OR DELETE ON crypto_restore_runs
    FOR EACH ROW EXECUTE FUNCTION guard_restore_progress();
CREATE TRIGGER crypto_restore_documents_guard BEFORE UPDATE OR DELETE ON crypto_restore_documents
    FOR EACH ROW EXECUTE FUNCTION guard_restore_progress();
CREATE TRIGGER crypto_restore_state_guard BEFORE UPDATE OR DELETE ON crypto_restore_state
    FOR EACH ROW EXECUTE FUNCTION guard_restore_progress();
CREATE TRIGGER crypto_restore_runs_no_truncate BEFORE TRUNCATE ON crypto_restore_runs
    FOR EACH STATEMENT EXECUTE FUNCTION reject_journal_mutation();
CREATE TRIGGER crypto_restore_documents_no_truncate BEFORE TRUNCATE ON crypto_restore_documents
    FOR EACH STATEMENT EXECUTE FUNCTION reject_journal_mutation();
CREATE TRIGGER crypto_restore_state_no_truncate BEFORE TRUNCATE ON crypto_restore_state
    FOR EACH STATEMENT EXECUTE FUNCTION reject_journal_mutation();
