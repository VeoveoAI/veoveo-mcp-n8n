-- Record that a run's Task dispatched the execution, for the Task's caller. A dispatch
-- that found the execution again records it once.
INSERT INTO n8n.run_execution (tenant, instance_id, execution_id, task_id, caller, dispatched_at)
VALUES ($1, $2, $3, $4, $5, now())
ON CONFLICT (tenant, instance_id, execution_id) DO NOTHING
