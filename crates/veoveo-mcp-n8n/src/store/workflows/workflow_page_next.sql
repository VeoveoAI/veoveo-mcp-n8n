-- A page of a tenant's applied workflows after a cursor's name. A separate statement
-- for the first page keeps this bound an index condition in a cached generic plan.
SELECT workflow, n8n_workflow_id, file_sha256, n8n_version_id, runnable, applied
FROM n8n.applied_workflow
WHERE tenant = $1 AND workflow > $2
ORDER BY workflow
LIMIT $3
