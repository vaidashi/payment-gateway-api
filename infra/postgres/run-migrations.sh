#!/usr/bin/env sh
set -eu

until pg_isready --dbname "$APP_DATABASE_URL" >/dev/null 2>&1; do sleep 1; done
psql --dbname "$APP_DATABASE_URL" --set ON_ERROR_STOP=1 --file /migrations/0000_runtime.sql

until pg_isready --dbname "$PROVIDER_DATABASE_URL" >/dev/null 2>&1; do sleep 1; done
psql --dbname "$PROVIDER_DATABASE_URL" --set ON_ERROR_STOP=1 --file /provider-migrations/0000_runtime.sql
