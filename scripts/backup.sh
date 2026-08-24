#!/bin/sh

#!/bin/sh
set -eu

# Every backup service supplies a distinct label. n8n retains its existing
# environment names; Immich uses the BACKUP_* values supplied by its service.
BACKUP_LABEL="${BACKUP_LABEL:-n8n}"
BACKUP_DATABASE="${BACKUP_DATABASE:-${PGDATABASE:-}}"
BACKUP_USER="${BACKUP_USER:-${PGUSER:-}}"
BACKUP_PASSWORD="${BACKUP_PASSWORD:-${PGPASSWORD:-${DB_PASSWORD:-}}}"
BACKUP_HOST="${PGHOST:-}"
BACKUP_PORT="${PGPORT:-5432}"

: "${BACKUP_DIR:?BACKUP_DIR must be set}"
: "${BACKUP_DATABASE:?Database name must be set}"
: "${BACKUP_USER:?Database user must be set}"
: "${BACKUP_HOST:?Database host must be set}"

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
BACKUP_FILE="${BACKUP_DIR}/${BACKUP_LABEL}_backup_${TIMESTAMP}.dump"

# --- 1. Perform Backup ---
echo "Starting PostgreSQL backup for ${BACKUP_LABEL} at $TIMESTAMP..."

# Use pg_dump to create a custom-format backup file.
if PGPASSWORD="$BACKUP_PASSWORD" pg_dump -Fc -h "$BACKUP_HOST" -p "$BACKUP_PORT" -U "$BACKUP_USER" -d "$BACKUP_DATABASE" > "$BACKUP_FILE"; then
  echo "Backup successful! File created: $BACKUP_FILE"
else
  rm -f "$BACKUP_FILE"
  echo "Backup FAILED for ${BACKUP_LABEL}!"
  exit 1
fi

# Define the retention period (30 days for approximately one month).
RETENTION_DAYS=30
echo "Cleaning up ${BACKUP_LABEL} backups older than $RETENTION_DAYS days..."
find "$BACKUP_DIR" -type f -name "${BACKUP_LABEL}_backup_*.dump" -mtime +"$RETENTION_DAYS" -exec rm {} \;
echo "Cleanup complete."

echo "Next backup in 24 hours."
