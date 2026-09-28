-- Development-only named accounts keep local document access across cookie expiry.
-- Production authentication does not consult this table.
CREATE TABLE development_accounts (
 account text PRIMARY KEY CHECK(account ~ '^[A-Za-z0-9_-]{1,32}$'),
 user_id uuid NOT NULL REFERENCES users(user_id),
 workspace_id uuid NOT NULL REFERENCES workspaces(workspace_id),
 FOREIGN KEY(workspace_id,user_id) REFERENCES memberships(workspace_id,user_id)
);
