-- n8n's encryption key, beside the data it encrypts: n8n's tables in this schema outlive
-- the sandbox's disk, so the key does too. Every credential n8n holds is a provider's
-- placeholder, so the key protects no real secret. `instance_id` names this n8n data
-- set: it is created with the row, so n8n's data started again over a reset schema is
-- another instance, and its execution numbers, which start again at 1, never meet an
-- earlier instance's in the runs' recorded jobs.
CREATE TABLE n8n.instance_key (
    tenant uuid NOT NULL,
    instance_id uuid NOT NULL,
    encryption_key text NOT NULL CHECK (char_length(encryption_key) BETWEEN 32 AND 256),
    created_at timestamptz NOT NULL,
    PRIMARY KEY (tenant)
);

COMMENT ON TABLE n8n.instance_key IS
    'Retention: for the life of n8n''s data in this schema, which the key decrypts.';
