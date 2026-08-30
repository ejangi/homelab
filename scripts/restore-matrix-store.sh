#!/bin/sh

# Recover Matrix client and encryption state from a verified local archive.
# Usage: ./scripts/restore-matrix-store.sh backups/matrix_store_backup_....tar.gz
set -eu

if [ "$#" -ne 1 ]; then
  echo "Usage: $0 backups/matrix_store_backup_YYYYMMDD_HHMMSS.tar.gz" >&2
  exit 1
fi

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PROJECT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
BACKUP_DIR="$PROJECT_DIR/backups"
ARCHIVE=$(CDPATH= cd -- "$(dirname -- "$1")" && pwd)/$(basename -- "$1")

case "$ARCHIVE" in
  "$BACKUP_DIR"/matrix_store_backup_*.tar.gz) ;;
  *)
    echo "Archive must be a Matrix backup below $BACKUP_DIR" >&2
    exit 1
    ;;
esac

if [ ! -f "$ARCHIVE" ] || [ ! -f "${ARCHIVE}.sha256" ]; then
  echo "Archive or checksum file is missing." >&2
  exit 1
fi

ARCHIVE_NAME=$(basename -- "$ARCHIVE")
restart_services() {
  docker compose -f "$PROJECT_DIR/compose.yml" up -d matrix matrix_store_backup >/dev/null
}

echo "Stopping Matrix delivery while the encrypted SQLite state is restored..."
docker compose -f "$PROJECT_DIR/compose.yml" stop matrix matrix_store_backup >/dev/null
trap restart_services EXIT INT TERM
docker compose -f "$PROJECT_DIR/compose.yml" --profile maintenance run --rm --no-deps \
  -e "MATRIX_STORE_BACKUP_ARCHIVE=/backups/$ARCHIVE_NAME" matrix_store_restore
trap - EXIT INT TERM
restart_services
echo "Matrix service restarted. Verify encrypted delivery before relying on the restored state."
