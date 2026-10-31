-- Keep the encryption key of the tenant's n8n with a new instance identity, unless one is
-- kept already.
INSERT INTO n8n.instance_key (tenant, instance_id, encryption_key, created_at)
VALUES ($1, $2, $3, now())
ON CONFLICT (tenant) DO NOTHING
