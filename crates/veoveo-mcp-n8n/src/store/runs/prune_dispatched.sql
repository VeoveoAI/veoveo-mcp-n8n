-- Forget the dispatched executions n8n no longer keeps, after its default pruning of 336
-- hours, and those of another n8n instance, once at each start.
-- veoveo: unbounded (one tenant's rows of two weeks of runs, deleted once per start)
DELETE FROM n8n.run_execution
WHERE tenant = $1 AND (instance_id <> $2 OR dispatched_at < now() - interval '336 hours')
