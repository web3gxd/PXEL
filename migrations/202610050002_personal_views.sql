CREATE TABLE personal_document_views (
    document_id uuid NOT NULL REFERENCES documents(document_id),
    user_id uuid NOT NULL REFERENCES users(user_id),
    revision bigint NOT NULL CHECK (revision > 0),
    payload bytea NOT NULL CHECK (octet_length(payload) BETWEEN 57 AND 131128),
    PRIMARY KEY (document_id, user_id)
);
