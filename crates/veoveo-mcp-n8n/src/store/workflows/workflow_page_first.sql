-- The first page of a tenant's applied workflows in name order, with one row of
-- lookahead.
SELECT workflow, n8n_workflow_id, file_sha256, n8n_version_id, runnable, applied
FROM n8n.applied_workflow
WHERE tenant = $1
ORDER BY workflow
LIMIT $2
