CREATE TABLE document_metadata (
 document_id uuid PRIMARY KEY REFERENCES documents(document_id),
 revision bigint NOT NULL CHECK(revision>0),
 payload bytea NOT NULL CHECK(octet_length(payload)<=8320)
);
CREATE TABLE document_library_preferences (
 document_id uuid NOT NULL REFERENCES documents(document_id),
 user_id uuid NOT NULL REFERENCES users(user_id),
 revision bigint NOT NULL CHECK(revision>0),
 payload bytea NOT NULL CHECK(octet_length(payload)<=4224),
 PRIMARY KEY(document_id,user_id)
);
CREATE TABLE library_layout_preferences (
 workspace_id uuid NOT NULL,
 user_id uuid NOT NULL,
 revision bigint NOT NULL CHECK(revision>0),
 layout text NOT NULL CHECK(layout IN ('cards','list')),
 PRIMARY KEY(workspace_id,user_id),
 FOREIGN KEY(workspace_id,user_id) REFERENCES memberships(workspace_id,user_id)
);
