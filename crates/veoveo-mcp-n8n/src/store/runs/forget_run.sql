-- Forget a run whose Task ended.
DELETE FROM n8n.run
WHERE tenant = $1 AND task_id = $2
