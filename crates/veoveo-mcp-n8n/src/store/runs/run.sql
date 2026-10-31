-- One run of a tenant by its Task.
SELECT task_id, workflow, operation_id, execution_id
FROM n8n.run
WHERE tenant = $1 AND task_id = $2
