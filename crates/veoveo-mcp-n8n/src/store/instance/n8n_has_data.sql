-- Whether n8n's migrations ran in the server's schema: their table exists. The statement
-- reads the catalog, which names no tenant and holds one row for the relation at most.
SELECT pg_catalog.to_regclass('n8n.n8n_migrations') IS NOT NULL AS "exists!"
