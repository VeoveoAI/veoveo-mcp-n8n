-- One applied workflow of a tenant by its name.
SELECT workflow, n8n_workflow_id, file_sha256, n8n_version_id, runnable, applied
FROM n8n.applied_workflow
WHERE tenant = $1 AND workflow = $2
