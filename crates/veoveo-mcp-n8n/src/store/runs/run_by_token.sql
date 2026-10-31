-- The run whose token has this SHA-256, for the bridge.
SELECT task_id, workflow, operation_id, execution_id
FROM n8n.run
WHERE tenant = $1 AND token_sha256 = $2
