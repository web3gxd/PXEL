-- Keys are committed before genesis; no FK to documents may erase that ordering.
CREATE TABLE document_crypto_keys (
    document_id uuid NOT NULL,
    workspace_id uuid NOT NULL,
    dek_generation integer NOT NULL CHECK (dek_generation > 0),
    physical_key_id bytea NOT NULL CHECK (octet_length(physical_key_id) = 32),
    active boolean NOT NULL DEFAULT true,
    kek_id text NOT NULL CHECK (length(kek_id) BETWEEN 1 AND 64),
    wrapped_dek bytea NOT NULL CHECK (octet_length(wrapped_dek) = 48),
    wrapping_nonce bytea NOT NULL CHECK (octet_length(wrapping_nonce) = 24),
    wrap_schema_version integer NOT NULL CHECK (wrap_schema_version = 1),
    wrap_revision bigint NOT NULL DEFAULT 1 CHECK (wrap_revision > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (document_id, dek_generation)
);
CREATE UNIQUE INDEX document_crypto_keys_one_active
    ON document_crypto_keys(document_id) WHERE active;

CREATE TABLE crypto_nonce_reservations (
    physical_key_id bytea NOT NULL CHECK (octet_length(physical_key_id) = 32),
    nonce bytea NOT NULL CHECK (octet_length(nonce) = 24),
    reserved_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (physical_key_id, nonce)
);
CREATE TRIGGER crypto_nonce_reservations_immutable
    BEFORE UPDATE OR DELETE ON crypto_nonce_reservations
    FOR EACH ROW EXECUTE FUNCTION reject_journal_mutation();
CREATE TRIGGER crypto_nonce_reservations_no_truncate
    BEFORE TRUNCATE ON crypto_nonce_reservations
    FOR EACH STATEMENT EXECUTE FUNCTION reject_journal_mutation();

CREATE FUNCTION guard_document_crypto_key() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'document keys retain readable generations' USING ERRCODE = '55000';
    END IF;
    IF NEW.document_id IS DISTINCT FROM OLD.document_id
       OR NEW.workspace_id IS DISTINCT FROM OLD.workspace_id
       OR NEW.dek_generation IS DISTINCT FROM OLD.dek_generation
       OR NEW.physical_key_id IS DISTINCT FROM OLD.physical_key_id
       OR NEW.created_at IS DISTINCT FROM OLD.created_at
    THEN
        RAISE EXCEPTION 'document key identity is immutable' USING ERRCODE = '55000';
    END IF;
    IF (NEW.kek_id IS DISTINCT FROM OLD.kek_id
        OR NEW.wrapped_dek IS DISTINCT FROM OLD.wrapped_dek
        OR NEW.wrapping_nonce IS DISTINCT FROM OLD.wrapping_nonce
        OR NEW.wrap_schema_version IS DISTINCT FROM OLD.wrap_schema_version)
       AND NEW.wrap_revision <> OLD.wrap_revision + 1
    THEN
        RAISE EXCEPTION 'key rewrap revision must advance' USING ERRCODE = '55000';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER document_crypto_keys_guard
    BEFORE UPDATE OR DELETE ON document_crypto_keys
    FOR EACH ROW EXECUTE FUNCTION guard_document_crypto_key();
CREATE TRIGGER document_crypto_keys_no_truncate
    BEFORE TRUNCATE ON document_crypto_keys
    FOR EACH STATEMENT EXECUTE FUNCTION reject_journal_mutation();

-- Existing rows remain byte-for-byte unchanged. Runtime rejects a plaintext
-- dataset until a separately authorized offline migration has been verified.
ALTER TABLE document_operations DROP CONSTRAINT document_operations_payload_check;
ALTER TABLE document_operations ADD CONSTRAINT document_operations_payload_check
    CHECK (octet_length(payload) <= 4194360);
ALTER TABLE document_operations DROP CONSTRAINT document_operations_command_fingerprint_check;
ALTER TABLE document_operations ADD CONSTRAINT document_operations_command_fingerprint_check
    CHECK (octet_length(command_fingerprint) IN (32, 88));
ALTER TABLE document_operations DROP CONSTRAINT document_operations_canonical_event_hash_check;
ALTER TABLE document_operations ADD CONSTRAINT document_operations_canonical_event_hash_check
    CHECK (octet_length(canonical_event_hash) IN (32, 88));
ALTER TABLE document_snapshots DROP CONSTRAINT document_snapshots_byte_length_check;
ALTER TABLE document_snapshots ADD CONSTRAINT document_snapshots_byte_length_check
    CHECK (byte_length > 0 AND byte_length <= 33554488);
ALTER TABLE document_snapshots DROP CONSTRAINT document_snapshots_state_hash_check;
ALTER TABLE document_snapshots ADD CONSTRAINT document_snapshots_state_hash_check
    CHECK (octet_length(state_hash) IN (32, 88));
ALTER TABLE export_audit_events DROP CONSTRAINT export_audit_events_payload_checksum_check;
ALTER TABLE export_audit_events ADD CONSTRAINT export_audit_events_payload_checksum_check
    CHECK (octet_length(payload_checksum) IN (32, 96));
ALTER TABLE export_audit_events DROP CONSTRAINT export_audit_events_payload_size_check;
ALTER TABLE export_audit_events ADD CONSTRAINT export_audit_events_payload_size_check
    CHECK ((octet_length(payload_checksum) = 32 AND payload_size > 0 AND payload_size <= 33554432)
        OR (octet_length(payload_checksum) = 96 AND payload_size = 0));

CREATE FUNCTION is_encrypted_storage_envelope(body bytea) RETURNS boolean
LANGUAGE sql IMMUTABLE STRICT AS $$
    SELECT octet_length(body) >= 56
        AND substring(body FROM 1 FOR 12) = decode('5058454c454e430000010001', 'hex')
        AND substring(body FROM 13 FOR 4) <> decode('00000000', 'hex');
$$;

CREATE FUNCTION guard_encrypted_storage_insert() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME = 'document_operations' THEN
        IF NOT is_encrypted_storage_envelope(NEW.payload)
           OR NOT is_encrypted_storage_envelope(NEW.command_fingerprint)
           OR NOT is_encrypted_storage_envelope(NEW.canonical_event_hash)
        THEN
            RAISE EXCEPTION 'encrypted journal envelopes are required' USING ERRCODE = '23514';
        END IF;
    ELSIF TG_TABLE_NAME = 'document_snapshots' THEN
        IF NOT is_encrypted_storage_envelope(NEW.state_hash) THEN
            RAISE EXCEPTION 'encrypted snapshot metadata is required' USING ERRCODE = '23514';
        END IF;
    ELSIF TG_TABLE_NAME = 'export_audit_events' THEN
        IF NOT is_encrypted_storage_envelope(NEW.payload_checksum) OR NEW.payload_size <> 0 THEN
            RAISE EXCEPTION 'encrypted export metadata is required' USING ERRCODE = '23514';
        END IF;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER document_operations_encrypted_insert
    BEFORE INSERT ON document_operations FOR EACH ROW EXECUTE FUNCTION guard_encrypted_storage_insert();
CREATE TRIGGER document_snapshots_encrypted_insert
    BEFORE INSERT ON document_snapshots FOR EACH ROW EXECUTE FUNCTION guard_encrypted_storage_insert();
CREATE TRIGGER export_audit_events_encrypted_insert
    BEFORE INSERT ON export_audit_events FOR EACH ROW EXECUTE FUNCTION guard_encrypted_storage_insert();
