CREATE TABLE orders (
  id UUID PRIMARY KEY,
  customer_id UUID NOT NULL REFERENCES users(id),
  restaurant_id UUID NOT NULL REFERENCES restaurants(id),
  status TEXT NOT NULL CHECK (status IN ('PLACED','PAID','IN_PROGRESS','READY','COMPLETED','CANCELLED')),
  configuration_version BIGINT NOT NULL,
  tax_basis_points INTEGER NOT NULL CHECK (tax_basis_points >= 0),
  service_fee_basis_points INTEGER NOT NULL CHECK (service_fee_basis_points BETWEEN 0 AND 300),
  subtotal_cents BIGINT NOT NULL CHECK (subtotal_cents >= 0),
  tax_cents BIGINT NOT NULL CHECK (tax_cents >= 0),
  service_fee_cents BIGINT NOT NULL CHECK (service_fee_cents >= 0),
  total_cents BIGINT NOT NULL CHECK (total_cents >= 0),
  version BIGINT NOT NULL DEFAULT 1,
  placed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  paid_at TIMESTAMPTZ,
  in_progress_at TIMESTAMPTZ,
  ready_at TIMESTAMPTZ,
  completed_at TIMESTAMPTZ,
  cancelled_at TIMESTAMPTZ
);
CREATE INDEX orders_customer_created_idx ON orders(customer_id, placed_at DESC, id DESC);
CREATE INDEX orders_restaurant_status_created_idx ON orders(restaurant_id, status, placed_at, id);

CREATE TABLE order_lines (
  id UUID PRIMARY KEY,
  order_id UUID NOT NULL REFERENCES orders(id),
  menu_item_id UUID NOT NULL,
  item_name TEXT NOT NULL,
  unit_price_cents BIGINT NOT NULL CHECK (unit_price_cents >= 0),
  quantity INTEGER NOT NULL CHECK (quantity BETWEEN 1 AND 99),
  line_total_cents BIGINT NOT NULL CHECK (line_total_cents >= 0)
);
CREATE TABLE order_line_options (
  id UUID PRIMARY KEY,
  order_line_id UUID NOT NULL REFERENCES order_lines(id),
  menu_option_id UUID NOT NULL,
  option_name TEXT NOT NULL,
  price_adjustment_cents BIGINT NOT NULL CHECK (price_adjustment_cents >= 0),
  kind TEXT NOT NULL CHECK (kind IN ('selection','extra'))
);

CREATE TABLE command_identities (
  actor_id UUID NOT NULL REFERENCES users(id),
  operation TEXT NOT NULL,
  idempotency_key UUID NOT NULL,
  input_digest TEXT NOT NULL,
  resource_id UUID NOT NULL,
  response_status INTEGER NOT NULL,
  response_body JSONB NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (actor_id, operation, idempotency_key)
);
