-- Foundation only. The durable operation codec/log and CAS use case begin in phase 1.
CREATE TABLE users (
    user_id uuid PRIMARY KEY,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE workspaces (
    workspace_id uuid PRIMARY KEY,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE memberships (
    workspace_id uuid NOT NULL REFERENCES workspaces(workspace_id),
    user_id uuid NOT NULL REFERENCES users(user_id),
    role text NOT NULL CHECK (role IN ('owner', 'member')),
    access_epoch bigint NOT NULL DEFAULT 0 CHECK (access_epoch >= 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, user_id)
);

CREATE INDEX memberships_user_id_idx ON memberships(user_id);

CREATE TABLE documents (
    document_id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES workspaces(workspace_id),
    head_version bigint NOT NULL DEFAULT 0 CHECK (head_version >= 0),
    access_epoch bigint NOT NULL DEFAULT 0 CHECK (access_epoch >= 0),
    state text NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'quarantined')),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, document_id)
);

CREATE TABLE document_permissions (
    workspace_id uuid NOT NULL,
    document_id uuid NOT NULL,
    user_id uuid NOT NULL,
    role text NOT NULL CHECK (role IN ('owner', 'editor', 'viewer')),
    PRIMARY KEY (document_id, user_id),
    FOREIGN KEY (workspace_id, document_id) REFERENCES documents(workspace_id, document_id),
    FOREIGN KEY (workspace_id, user_id) REFERENCES memberships(workspace_id, user_id)
);

CREATE INDEX document_permissions_user_id_idx ON document_permissions(user_id);
