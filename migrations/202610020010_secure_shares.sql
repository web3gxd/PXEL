ALTER TABLE users ADD COLUMN identity_kind text NOT NULL DEFAULT 'member'
    CHECK (identity_kind IN ('member','share_guest'));
CREATE FUNCTION guard_user_identity_kind() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.identity_kind IS DISTINCT FROM OLD.identity_kind THEN
        RAISE EXCEPTION 'user identity kind is immutable' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER users_identity_kind_immutable BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION guard_user_identity_kind();

CREATE FUNCTION require_member_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS(SELECT 1 FROM users WHERE user_id=NEW.user_id AND identity_kind='member') THEN
        RAISE EXCEPTION 'member identity required' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER memberships_member_identity BEFORE INSERT OR UPDATE ON memberships
    FOR EACH ROW EXECUTE FUNCTION require_member_identity();
CREATE TRIGGER document_permissions_member_identity BEFORE INSERT OR UPDATE ON document_permissions
    FOR EACH ROW EXECUTE FUNCTION require_member_identity();
CREATE TRIGGER development_accounts_member_identity BEFORE INSERT OR UPDATE ON development_accounts
    FOR EACH ROW EXECUTE FUNCTION require_member_identity();
CREATE TRIGGER development_sessions_member_identity BEFORE INSERT OR UPDATE ON development_sessions
    FOR EACH ROW EXECUTE FUNCTION require_member_identity();

CREATE TABLE share_links (
    link_id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL,
    document_id uuid NOT NULL,
    owner_id uuid NOT NULL REFERENCES users(user_id),
    create_request_id uuid NOT NULL UNIQUE,
    token_hash bytea NOT NULL UNIQUE CHECK(octet_length(token_hash)=32),
    role text NOT NULL CHECK(role IN ('view','comment','edit')),
    secret_kind text NOT NULL CHECK(secret_kind IN ('pin','password')),
    secret_phc bytea NOT NULL CHECK(is_encrypted_storage_envelope(secret_phc) AND octet_length(secret_phc)<=312),
    secret_intent bytea NOT NULL CHECK(is_encrypted_storage_envelope(secret_intent) AND octet_length(secret_intent)=88),
    status text NOT NULL DEFAULT 'active' CHECK(status IN ('active','revoked','expired','disabled')),
    failed_attempts integer NOT NULL DEFAULT 0 CHECK(failed_attempts BETWEEN 0 AND 4),
    lockouts integer NOT NULL DEFAULT 0 CHECK(lockouts BETWEEN 0 AND 3),
    blocked_until timestamptz,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    expires_at timestamptz,
    FOREIGN KEY(workspace_id,document_id) REFERENCES documents(workspace_id,document_id),
    UNIQUE(link_id,document_id),
    CHECK(expires_at IS NULL OR expires_at>created_at),
    CHECK(blocked_until IS NULL OR (status='active' AND lockouts BETWEEN 1 AND 2)),
    CHECK(status<>'disabled' OR lockouts=3)
);
CREATE INDEX share_links_document_created ON share_links(document_id,created_at,link_id);
CREATE INDEX share_links_expiry ON share_links(expires_at) WHERE status='active' AND expires_at IS NOT NULL;

CREATE TABLE share_guest_sessions (
    session_id uuid PRIMARY KEY,
    link_id uuid NOT NULL REFERENCES share_links(link_id),
    document_id uuid NOT NULL REFERENCES documents(document_id),
    user_id uuid NOT NULL UNIQUE REFERENCES users(user_id),
    token_hash bytea NOT NULL UNIQUE CHECK(octet_length(token_hash)=32),
    admitted_head_version bigint NOT NULL CHECK(admitted_head_version>=0),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    FOREIGN KEY(link_id,document_id) REFERENCES share_links(link_id,document_id),
    CHECK(expires_at>created_at AND expires_at<=created_at+interval '24 hours')
);
CREATE INDEX share_guest_sessions_link ON share_guest_sessions(link_id);

CREATE TABLE share_audit_events (
    event_id uuid PRIMARY KEY,
    link_id uuid NOT NULL REFERENCES share_links(link_id),
    document_id uuid NOT NULL REFERENCES documents(document_id),
    actor_id uuid REFERENCES users(user_id),
    request_id uuid,
    event_type text NOT NULL CHECK(event_type IN ('created','revoked','expired','authenticated','authentication_failed','blocked','disabled','restored')),
    access_epoch bigint NOT NULL CHECK(access_epoch>=0),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    FOREIGN KEY(link_id,document_id) REFERENCES share_links(link_id,document_id)
);
CREATE UNIQUE INDEX share_audit_requests ON share_audit_events(actor_id,request_id) WHERE request_id IS NOT NULL;
CREATE INDEX share_audit_document_created ON share_audit_events(document_id,created_at);
CREATE TRIGGER share_audit_events_immutable BEFORE UPDATE OR DELETE ON share_audit_events
    FOR EACH ROW EXECUTE FUNCTION reject_journal_mutation();
CREATE TRIGGER share_audit_events_no_truncate BEFORE TRUNCATE ON share_audit_events
    FOR EACH STATEMENT EXECUTE FUNCTION reject_journal_mutation();

CREATE TABLE share_owner_notifications (
    notification_id uuid PRIMARY KEY,
    event_id uuid NOT NULL REFERENCES share_audit_events(event_id),
    user_id uuid NOT NULL REFERENCES users(user_id),
    document_id uuid NOT NULL REFERENCES documents(document_id),
    link_id uuid NOT NULL REFERENCES share_links(link_id),
    reason text NOT NULL CHECK(reason='disabled'),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(event_id,user_id)
);
CREATE INDEX share_owner_notifications_user ON share_owner_notifications(user_id,created_at,notification_id);
CREATE TRIGGER share_owner_notifications_member_identity BEFORE INSERT OR UPDATE ON share_owner_notifications
    FOR EACH ROW EXECUTE FUNCTION require_member_identity();
CREATE TRIGGER share_owner_notifications_immutable BEFORE UPDATE OR DELETE ON share_owner_notifications
    FOR EACH ROW EXECUTE FUNCTION reject_journal_mutation();
CREATE TRIGGER share_owner_notifications_no_truncate BEFORE TRUNCATE ON share_owner_notifications
    FOR EACH STATEMENT EXECUTE FUNCTION reject_journal_mutation();

CREATE FUNCTION guard_share_link() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        RAISE EXCEPTION 'share identities are retained' USING ERRCODE='55000';
    END IF;
    IF TG_OP='INSERT' THEN
        IF NOT EXISTS(SELECT 1 FROM users WHERE user_id=NEW.owner_id AND identity_kind='member') THEN
            RAISE EXCEPTION 'share owner must be a member' USING ERRCODE='23514';
        END IF;
    ELSIF NEW.link_id IS DISTINCT FROM OLD.link_id OR NEW.workspace_id IS DISTINCT FROM OLD.workspace_id
        OR NEW.document_id IS DISTINCT FROM OLD.document_id OR NEW.owner_id IS DISTINCT FROM OLD.owner_id
        OR NEW.create_request_id IS DISTINCT FROM OLD.create_request_id OR NEW.token_hash IS DISTINCT FROM OLD.token_hash
        OR NEW.role IS DISTINCT FROM OLD.role OR NEW.secret_kind IS DISTINCT FROM OLD.secret_kind
        OR NEW.secret_phc IS DISTINCT FROM OLD.secret_phc OR NEW.secret_intent IS DISTINCT FROM OLD.secret_intent
        OR NEW.created_at IS DISTINCT FROM OLD.created_at OR NEW.expires_at IS DISTINCT FROM OLD.expires_at
        OR OLD.status<>'active' THEN
        RAISE EXCEPTION 'share identity and terminal state are immutable' USING ERRCODE='55000';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER share_links_guard BEFORE INSERT OR UPDATE OR DELETE ON share_links
    FOR EACH ROW EXECUTE FUNCTION guard_share_link();
CREATE TRIGGER share_links_no_truncate BEFORE TRUNCATE ON share_links
    FOR EACH STATEMENT EXECUTE FUNCTION reject_journal_mutation();

CREATE FUNCTION guard_share_guest_session() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        RAISE EXCEPTION 'guest authors are retained' USING ERRCODE='55000';
    END IF;
    IF TG_OP='INSERT' THEN
        IF NOT EXISTS(SELECT 1 FROM users WHERE user_id=NEW.user_id AND identity_kind='share_guest') THEN
            RAISE EXCEPTION 'guest session requires guest identity' USING ERRCODE='23514';
        END IF;
    ELSIF NEW.session_id IS DISTINCT FROM OLD.session_id OR NEW.link_id IS DISTINCT FROM OLD.link_id
        OR NEW.document_id IS DISTINCT FROM OLD.document_id
        OR NEW.user_id IS DISTINCT FROM OLD.user_id OR NEW.token_hash IS DISTINCT FROM OLD.token_hash
        OR NEW.admitted_head_version IS DISTINCT FROM OLD.admitted_head_version
        OR NEW.created_at IS DISTINCT FROM OLD.created_at OR NEW.expires_at IS DISTINCT FROM OLD.expires_at
        OR OLD.revoked_at IS NOT NULL OR NEW.revoked_at IS NULL THEN
        RAISE EXCEPTION 'guest session identity is immutable' USING ERRCODE='55000';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER share_guest_sessions_guard BEFORE INSERT OR UPDATE OR DELETE ON share_guest_sessions
    FOR EACH ROW EXECUTE FUNCTION guard_share_guest_session();
CREATE TRIGGER share_guest_sessions_no_truncate BEFORE TRUNCATE ON share_guest_sessions
    FOR EACH STATEMENT EXECUTE FUNCTION reject_journal_mutation();

CREATE FUNCTION guard_guest_operation_lineage() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.actor_id IS NOT NULL AND EXISTS(SELECT 1 FROM users WHERE user_id=NEW.actor_id AND identity_kind='share_guest')
       AND NOT EXISTS(SELECT 1 FROM share_guest_sessions s JOIN share_links l ON l.link_id=s.link_id
            WHERE s.user_id=NEW.actor_id AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp()
              AND l.document_id=NEW.document_id AND l.status='active' AND l.role='edit'
              AND (l.expires_at IS NULL OR l.expires_at>clock_timestamp())
              AND NEW.committed_version>s.admitted_head_version) THEN
        RAISE EXCEPTION 'guest operation outside admitted lineage' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER document_operations_guest_lineage BEFORE INSERT ON document_operations
    FOR EACH ROW EXECUTE FUNCTION guard_guest_operation_lineage();
