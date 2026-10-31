-- Record the execution n8n's trigger started for a run.
UPDATE n8n.run
SET execution_id = $3
WHERE tenant = $1 AND task_id = $2
