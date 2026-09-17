# Mock Food Ordering

This repository starts with the local runtime foundation for a mock food-ordering system. It uses three independently startable Rust processes: `api`, `worker`, and `mock_provider`. PostgreSQL is the durable authority; Redis is disposable. The provider has separate PostgreSQL credentials and cannot be reached from the host or queried by app processes.

## Prerequisites

- Docker Compose 2+
- Rust 1.97.1 (pinned in `rust-toolchain.toml`)
- Node 22.12+ (Vite's documented minimum; Node 24.13.0 was verified locally)

## Run locally

```sh
cp .env.example .env
docker compose --project-name mock-food-ordering-dev up --build
```

The proxy is available at `http://localhost:8080`; Vite is also exposed at `http://localhost:5173` for development. API direct access is `http://localhost:8081`. PostgreSQL, Redis, the worker health port, and the mock provider are Compose-network-only.

Use a clean volume when testing initialization:

```sh
docker compose --project-name mock-food-ordering-dev down --volumes
docker compose --project-name mock-food-ordering-dev up --build
```

## Verification commands

```sh
cargo fmt --check
cargo build --locked --workspace
(cd web && npm ci && npm run build)
docker compose --project-name mock-food-ordering-dev config
```

To start each backend process without Compose, provide its required database URL first. `mock_provider` also requires `PROVIDER_SERVICE_TOKEN`. A missing value exits with a named configuration error. No real payment account, card data, or payment credential is used.

The API seeds four restaurants (12 items each), four customers, four restaurant-scoped staff accounts, and one admin on its first successful startup after migrations. Start with `GET /api/session` to receive a CSRF-bound anonymous cookie, list identities through `GET /api/demo/accounts`, then select one through `POST /api/demo/session` with the cookie, exact `Origin`, and `X-CSRF-Token` header. Quotes at `POST /api/orders/quote` use integer cents and basis points and always re-read PostgreSQL; Redis is not used as a monetary authority.

## Version pins

- Rust `1.97.1`: installed toolchain used to produce the lockfile; the workspace MSRV is `1.88`, matching Actix Web 4.15's documented floor.
- Actix Web `4.15.0`, SQLx `0.9.0`, Tokio `1.48.0`, serde `1.0.228`: exact runtime pins selected from the approved KTD1 ranges.
- React `19.2.3`, Vite `7.3.1`, TanStack Query `5.90.12`, and TanStack Router `1.131.36`: exact compatible SPA pins. Vite documents Node `22.12+` support.
- Compose pins the pulled PostgreSQL 18.0, Redis 8.0.3, and Caddy 2.10.2 images by digest. The backend pins its Rust 1.97.1 and Debian Bookworm bases; the web image pins Node 24.13.0 Bookworm. These digests were captured during the local Compose smoke test on `linux/arm64`.
