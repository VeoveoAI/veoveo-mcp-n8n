-- Record what the server applied for one workflow file: n8n's workflow, the file's
-- SHA-256, the n8n version it produced and the JSON it sent.
INSERT INTO n8n.applied_workflow
    (tenant, workflow, n8n_workflow_id, file_sha256, n8n_version_id, runnable, applied, applied_at)
VALUES ($1, $2, $3, $4, $5, $6, $7, now())
ON CONFLICT (tenant, workflow) DO UPDATE SET
    n8n_workflow_id = EXCLUDED.n8n_workflow_id,
    file_sha256 = EXCLUDED.file_sha256,
    n8n_version_id = EXCLUDED.n8n_version_id,
    runnable = EXCLUDED.runnable,
    applied = EXCLUDED.applied,
    applied_at = EXCLUDED.applied_at
