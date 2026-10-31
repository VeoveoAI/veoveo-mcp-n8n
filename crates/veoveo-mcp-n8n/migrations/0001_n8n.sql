-- The server's own tables in its schema. n8n's tables share the schema under the prefix
-- n8n_ and belong to n8n's migrations; nothing here names them.
--
-- One row for each workflow the server applied from the branch's workflows/ to n8n.
-- `workflow` uses the C collation, so its order is the byte order of the names, which
-- the listing's keyset relies on.
CREATE TABLE n8n.applied_workflow (
    tenant uuid NOT NULL,
    workflow text COLLATE "C" NOT NULL
        CHECK (char_length(workflow) <= 64 AND workflow ~ '^[a-z0-9]+(-[a-z0-9]+)*$'),
    n8n_workflow_id text NOT NULL CHECK (n8n_workflow_id ~ '^[A-Za-z0-9]{1,36}$'),
    file_sha256 bytea NOT NULL CHECK (octet_length(file_sha256) = 32),
    n8n_version_id uuid NOT NULL,
    runnable boolean NOT NULL,
    applied jsonb NOT NULL CHECK (jsonb_typeof(applied) = 'object'),
    applied_at timestamptz NOT NULL,
    PRIMARY KEY (tenant, workflow)
);

COMMENT ON TABLE n8n.applied_workflow IS
    'Retention: while the workflow''s file exists on the branch. A start archives the n8n workflow of a removed file and deletes its row.';
