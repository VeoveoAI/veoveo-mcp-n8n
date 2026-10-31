-- The encryption key and the instance identity of the tenant's n8n.
SELECT instance_id, encryption_key
FROM n8n.instance_key
WHERE tenant = $1
