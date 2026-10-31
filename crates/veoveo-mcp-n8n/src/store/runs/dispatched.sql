-- The run's Task that dispatched the execution, and its caller, if one did.
SELECT task_id, caller
FROM n8n.run_execution
WHERE tenant = $1 AND instance_id = $2 AND execution_id = $3
