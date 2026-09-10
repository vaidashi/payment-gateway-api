CREATE TABLE IF NOT EXISTS provider_runtime_migrations (
    version TEXT PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO provider_runtime_migrations (version) VALUES ('0000_runtime')
ON CONFLICT (version) DO NOTHING;
