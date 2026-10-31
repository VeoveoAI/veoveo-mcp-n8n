-- The applied workflow n8n knows by this ID.
SELECT workflow, n8n_workflow_id, file_sha256, n8n_version_id, runnable, applied
FROM n8n.applied_workflow
WHERE tenant = $1 AND n8n_workflow_id = $2
