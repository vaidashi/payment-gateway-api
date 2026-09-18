CREATE TABLE IF NOT EXISTS provider_intents (
    id TEXT PRIMARY KEY,
    amount_cents BIGINT NOT NULL CHECK (amount_cents >= 0),
    currency TEXT NOT NULL CHECK (currency = 'usd'),
    callback_url TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS provider_idempotency_keys (
    operation TEXT NOT NULL,
    idempotency_key UUID NOT NULL,
    input_digest TEXT NOT NULL,
    response JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (operation, idempotency_key)
);

CREATE TABLE IF NOT EXISTS provider_attempts (
    id TEXT PRIMARY KEY,
    intent_id TEXT NOT NULL REFERENCES provider_intents(id),
    scenario TEXT NOT NULL CHECK (scenario IN ('success', 'decline', 'delayed_success', 'duplicate_callback')),
    outcome TEXT NOT NULL CHECK (outcome IN ('pending', 'declined', 'succeeded')),
    available_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS one_pending_attempt_per_provider_intent
    ON provider_attempts(intent_id) WHERE outcome = 'pending';
CREATE INDEX IF NOT EXISTS due_provider_attempts
    ON provider_attempts(available_at) WHERE outcome = 'pending';

CREATE TABLE IF NOT EXISTS provider_captures (
    id TEXT PRIMARY KEY,
    intent_id TEXT NOT NULL UNIQUE REFERENCES provider_intents(id),
    attempt_id TEXT NOT NULL UNIQUE REFERENCES provider_attempts(id),
    amount_cents BIGINT NOT NULL CHECK (amount_cents >= 0),
    currency TEXT NOT NULL CHECK (currency = 'usd'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS provider_refunds (
    id TEXT PRIMARY KEY,
    capture_id TEXT NOT NULL UNIQUE REFERENCES provider_captures(id),
    amount_cents BIGINT NOT NULL CHECK (amount_cents >= 0),
    currency TEXT NOT NULL CHECK (currency = 'usd'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS provider_callback_events (
    id TEXT PRIMARY KEY,
    intent_id TEXT NOT NULL REFERENCES provider_intents(id),
    attempt_id TEXT NOT NULL REFERENCES provider_attempts(id),
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS provider_callback_deliveries (
    id UUID PRIMARY KEY,
    event_id TEXT NOT NULL REFERENCES provider_callback_events(id),
    callback_url TEXT NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    due_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at TIMESTAMPTZ,
    lease_token UUID,
    lease_expires_at TIMESTAMPTZ,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS due_provider_callback_deliveries
    ON provider_callback_deliveries(due_at) WHERE delivered_at IS NULL;
