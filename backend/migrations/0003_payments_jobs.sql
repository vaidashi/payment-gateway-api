CREATE TABLE payment_attempts (
  id UUID PRIMARY KEY,
  order_id UUID NOT NULL REFERENCES orders(id),
  provider_intent_id TEXT NOT NULL,
  provider_attempt_id TEXT NOT NULL UNIQUE,
  amount_cents BIGINT NOT NULL CHECK (amount_cents >= 0),
  currency TEXT NOT NULL CHECK (currency = 'usd'),
  status TEXT NOT NULL CHECK (status IN ('pending','declined','succeeded','unknown')),
  provider_capture_id TEXT UNIQUE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE(order_id, provider_attempt_id)
);
CREATE INDEX payment_attempts_reconcile_idx ON payment_attempts(status, updated_at) WHERE status IN ('pending','unknown');

CREATE TABLE payment_inbox (
  event_id TEXT PRIMARY KEY,
  raw_body BYTEA NOT NULL,
  received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  processed_at TIMESTAMPTZ,
  quarantined_at TIMESTAMPTZ,
  error_code TEXT
);

CREATE TABLE refund_obligations (
  id UUID PRIMARY KEY,
  order_id UUID NOT NULL REFERENCES orders(id),
  provider_intent_id TEXT NOT NULL,
  provider_capture_id TEXT NOT NULL UNIQUE,
  amount_cents BIGINT NOT NULL CHECK (amount_cents >= 0),
  currency TEXT NOT NULL CHECK (currency = 'usd'),
  status TEXT NOT NULL CHECK (status IN ('pending','succeeded','unknown','quarantined')),
  provider_refund_id TEXT UNIQUE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE durable_jobs (
  id UUID PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('capture','reconcile_attempt','refund','reconcile_refund','process_inbox')),
  dedupe_key TEXT NOT NULL UNIQUE,
  payload JSONB NOT NULL,
  due_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  lease_token UUID,
  lease_expires_at TIMESTAMPTZ,
  attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  completed_at TIMESTAMPTZ,
  quarantined_at TIMESTAMPTZ,
  last_error TEXT,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX durable_jobs_due_idx ON durable_jobs(due_at, id) WHERE completed_at IS NULL AND quarantined_at IS NULL;
