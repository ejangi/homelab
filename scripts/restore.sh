#!/bin/sh

set -eu

# Restore a PostgreSQL custom-format dump into the Compose postgres service.
# Usage: ./scripts/restore.sh [path/to/backup.dump]

COMPOSE="${COMPOSE:-docker compose}"
BACKUP_DIR="${BACKUP_DIR:-backups}"

if [ "$#" -gt 1 ]; then
  echo "Usage: $0 [path/to/backup.dump]" >&2
  exit 2
fi

if [ "$#" -eq 1 ]; then
  RESTORE_FILE=$1
else
  RESTORE_FILE=$(find "$BACKUP_DIR" -maxdepth 1 -type f -name 'n8n_backup_*.dump' -print | sort | tail -n 1)
fi

if [ -z "${RESTORE_FILE:-}" ] || [ ! -f "$RESTORE_FILE" ]; then
  echo "No PostgreSQL dump found in $BACKUP_DIR." >&2
  exit 1
fi

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
SAFETY_FILE="$BACKUP_DIR/n8n_pre_restore_${TIMESTAMP}.dump"

cleanup() {
  status=$?
  echo "Starting n8n services..."
  $COMPOSE up -d matrix n8n caddy postgres_backup >/dev/null || true
  exit "$status"
}
trap cleanup EXIT INT TERM

echo "Using restore file: $RESTORE_FILE"
echo "Stopping Matrix, n8n, and backup services while the database is replaced..."
$COMPOSE stop matrix n8n caddy postgres_backup >/dev/null || true

echo "Starting PostgreSQL..."
$COMPOSE up -d postgres >/dev/null

echo "Waiting for PostgreSQL to become ready..."
i=0
while ! $COMPOSE exec -T postgres sh -c 'pg_isready -U "$POSTGRES_USER" -d "$POSTGRES_DB"' >/dev/null 2>&1; do
  i=$((i + 1))
  if [ "$i" -ge 30 ]; then
    echo "PostgreSQL did not become ready in time." >&2
    exit 1
  fi
  sleep 2
done

echo "Creating safety backup of the current database: $SAFETY_FILE"
$COMPOSE exec -T postgres sh -c 'pg_dump -Fc -U "$POSTGRES_USER" -d "$POSTGRES_DB"' > "$SAFETY_FILE"

echo "Replacing the current database with the backup..."
$COMPOSE exec -T postgres sh -c 'dropdb --if-exists --force -U "$POSTGRES_USER" "$POSTGRES_DB" && createdb -U "$POSTGRES_USER" -O "$POSTGRES_USER" "$POSTGRES_DB"'

echo "Restoring $RESTORE_FILE..."
$COMPOSE exec -T postgres sh -c 'pg_restore --no-owner --exit-on-error -U "$POSTGRES_USER" -d "$POSTGRES_DB"' < "$RESTORE_FILE"

echo "Database restore completed successfully."
