-- Self-hosted identities are separate from explicitly insecure development auth.
CREATE TABLE local_accounts (
    user_id uuid PRIMARY KEY REFERENCES users(user_id),
    workspace_id uuid NOT NULL,
    username text NOT NULL UNIQUE CHECK (username ~ '^[a-z0-9_.-]{3,32}$'),
    password_phc text NOT NULL CHECK (octet_length(password_phc) BETWEEN 80 AND 256),
    is_master boolean NOT NULL DEFAULT false,
    enabled boolean NOT NULL DEFAULT true,
    must_change_password boolean NOT NULL DEFAULT true,
    credential_epoch bigint NOT NULL DEFAULT 0 CHECK (credential_epoch >= 0),
    failed_attempts integer NOT NULL DEFAULT 0 CHECK (failed_attempts BETWEEN 0 AND 5),
    failed_window_started_at timestamptz,
    locked_until timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (workspace_id,user_id) REFERENCES memberships(workspace_id,user_id),
    CHECK (NOT is_master OR (enabled AND NOT must_change_password))
);
CREATE UNIQUE INDEX local_accounts_single_master ON local_accounts(is_master) WHERE is_master;

CREATE TABLE local_instance (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    workspace_id uuid NOT NULL REFERENCES workspaces(workspace_id),
    master_user_id uuid NOT NULL REFERENCES local_accounts(user_id),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE account_sessions (
    token_hash bytea PRIMARY KEY CHECK (octet_length(token_hash)=32),
    user_id uuid NOT NULL REFERENCES local_accounts(user_id),
    credential_epoch bigint NOT NULL CHECK (credential_epoch >= 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL DEFAULT (now()+interval '8 hours'),
    CHECK (expires_at > created_at AND expires_at <= created_at+interval '8 hours')
);
CREATE INDEX account_sessions_user ON account_sessions(user_id);
CREATE INDEX account_sessions_expiry ON account_sessions(expires_at);

CREATE FUNCTION protect_local_master() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        IF OLD.is_master THEN
            RAISE EXCEPTION 'master account is immutable' USING ERRCODE='55000';
        END IF;
        RETURN OLD;
    END IF;
    IF NEW.user_id IS DISTINCT FROM OLD.user_id OR NEW.workspace_id IS DISTINCT FROM OLD.workspace_id
       OR NEW.is_master IS DISTINCT FROM OLD.is_master THEN
        RAISE EXCEPTION 'local account identity is immutable' USING ERRCODE='55000';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER local_master_immutable BEFORE UPDATE OR DELETE ON local_accounts
    FOR EACH ROW EXECUTE FUNCTION protect_local_master();
CREATE TRIGGER local_instance_immutable BEFORE UPDATE OR DELETE ON local_instance
    FOR EACH ROW EXECUTE FUNCTION reject_journal_mutation();
CREATE TRIGGER local_instance_no_truncate BEFORE TRUNCATE ON local_instance
    FOR EACH STATEMENT EXECUTE FUNCTION reject_journal_mutation();

CREATE FUNCTION enforce_local_document_workspace() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE instance_workspace uuid;
BEGIN
    SELECT workspace_id INTO instance_workspace FROM local_instance WHERE singleton;
    IF instance_workspace IS NOT NULL AND NEW.workspace_id<>instance_workspace THEN
        RAISE EXCEPTION 'document must belong to local instance' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER documents_local_workspace BEFORE INSERT ON documents
    FOR EACH ROW EXECUTE FUNCTION enforce_local_document_workspace();

-- The application inserts its creator's owner grant after the document. Insert
-- the master's grant only then, so a master-created book has no duplicate grant.
CREATE FUNCTION grant_local_master_document_owner() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE instance_master uuid;
BEGIN
    SELECT master_user_id INTO instance_master FROM local_instance
        WHERE singleton AND workspace_id=NEW.workspace_id;
    IF NEW.role='owner' AND instance_master IS NOT NULL AND instance_master<>NEW.user_id THEN
        INSERT INTO document_permissions(workspace_id,document_id,user_id,role)
            VALUES(NEW.workspace_id,NEW.document_id,instance_master,'owner')
            ON CONFLICT(document_id,user_id) DO NOTHING;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER document_permissions_local_master AFTER INSERT ON document_permissions
    FOR EACH ROW EXECUTE FUNCTION grant_local_master_document_owner();
