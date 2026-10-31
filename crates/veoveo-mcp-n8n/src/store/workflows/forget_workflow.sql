-- Forget a workflow whose file left the branch, once n8n's copy is archived.
DELETE FROM n8n.applied_workflow
WHERE tenant = $1 AND workflow = $2
