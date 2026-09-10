CREATE TABLE users (
  id UUID PRIMARY KEY, display_name TEXT NOT NULL, role TEXT NOT NULL CHECK (role IN ('customer','staff','admin')), created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE restaurants (
  id UUID PRIMARY KEY, name TEXT NOT NULL, tax_basis_points INTEGER NOT NULL CHECK (tax_basis_points >= 0), service_fee_basis_points INTEGER CHECK (service_fee_basis_points BETWEEN 0 AND 300), configuration_version BIGINT NOT NULL DEFAULT 1, created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE staff_restaurants (user_id UUID NOT NULL REFERENCES users(id), restaurant_id UUID NOT NULL REFERENCES restaurants(id), PRIMARY KEY (user_id, restaurant_id));
CREATE TABLE menu_items (
  id UUID PRIMARY KEY, restaurant_id UUID NOT NULL REFERENCES restaurants(id), name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '', price_cents BIGINT NOT NULL CHECK (price_cents >= 0), available BOOLEAN NOT NULL DEFAULT true, category TEXT NOT NULL DEFAULT 'Menu', position INTEGER NOT NULL DEFAULT 0, CHECK (price_cents <= 1000000)
);
CREATE TABLE option_groups (id UUID PRIMARY KEY, menu_item_id UUID NOT NULL REFERENCES menu_items(id) ON DELETE CASCADE, name TEXT NOT NULL, required BOOLEAN NOT NULL, position INTEGER NOT NULL DEFAULT 0);
CREATE TABLE menu_options (id UUID PRIMARY KEY, option_group_id UUID NOT NULL REFERENCES option_groups(id) ON DELETE CASCADE, name TEXT NOT NULL, price_adjustment_cents BIGINT NOT NULL DEFAULT 0, position INTEGER NOT NULL DEFAULT 0, CHECK (price_adjustment_cents >= 0));
CREATE TABLE sessions (token_hash TEXT PRIMARY KEY, user_id UUID REFERENCES users(id), csrf_token TEXT NOT NULL, expires_at TIMESTAMPTZ NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now());
CREATE INDEX sessions_expiry_idx ON sessions(expires_at);
