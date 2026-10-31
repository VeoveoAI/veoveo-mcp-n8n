-- Every workflow the server applied for a tenant, in name order, bounded by the most a
-- team holds plus one row that tells a larger set apart.
SELECT workflow, n8n_workflow_id, file_sha256, n8n_version_id, runnable, applied
FROM n8n.applied_workflow
WHERE tenant = $1
ORDER BY workflow
LIMIT $2
