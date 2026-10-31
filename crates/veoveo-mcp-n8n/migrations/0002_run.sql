-- One row for each run the server dispatched to n8n for a Task: the workflow, the
-- operation, the SHA-256 of the run's token, by which the bridge finds the run, and n8n's
-- execution once the trigger answered.
CREATE TABLE n8n.run (
    tenant uuid NOT NULL,
    task_id uuid NOT NULL,
    workflow text COLLATE "C" NOT NULL
        CHECK (char_length(workflow) <= 64 AND workflow ~ '^[a-z0-9]+(-[a-z0-9]+)*$'),
    operation_id uuid NOT NULL,
    token_sha256 bytea NOT NULL CHECK (octet_length(token_sha256) = 32),
    execution_id text CHECK (execution_id ~ '^(0|[1-9][0-9]{0,18})$'),
    started_at timestamptz NOT NULL,
    PRIMARY KEY (tenant, task_id)
);

CREATE UNIQUE INDEX run_token ON n8n.run (tenant, token_sha256);

COMMENT ON TABLE n8n.run IS
    'Retention: while the run''s Task runs. The worker deletes the row when the Task ends.';

-- An execution names its n8n workflow; the server names it by its file.
CREATE UNIQUE INDEX applied_workflow_n8n_id ON n8n.applied_workflow (tenant, n8n_workflow_id);
