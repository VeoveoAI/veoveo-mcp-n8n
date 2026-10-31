-- Record a run's dispatch before the server calls n8n's trigger. A dispatch of the same
-- Task again, after an earlier one reached no execution, replaces its token.
INSERT INTO n8n.run (tenant, task_id, workflow, operation_id, token_sha256, execution_id, started_at)
VALUES ($1, $2, $3, $4, $5, NULL, now())
ON CONFLICT (tenant, task_id) DO UPDATE SET
    workflow = EXCLUDED.workflow,
    operation_id = EXCLUDED.operation_id,
    token_sha256 = EXCLUDED.token_sha256,
    execution_id = NULL,
    started_at = EXCLUDED.started_at
