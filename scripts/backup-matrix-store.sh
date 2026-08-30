#!/bin/sh

# Create online-consistent copies of the Matrix SDK SQLite state. The source
# volume is mounted read-only; SQLite's backup API includes committed WAL data
# without stopping the Matrix sender.
set -eu

MATRIX_STORE_DIR=${MATRIX_STORE_DIR:-/matrix-store}
BACKUP_DIR=${BACKUP_DIR:-/backups}
RETENTION_DAYS=${RETENTION_DAYS:-30}
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
WORK_DIR=$(mktemp -d "${BACKUP_DIR}/.matrix-store-${TIMESTAMP}.XXXXXX")
ARCHIVE="${BACKUP_DIR}/matrix_store_backup_${TIMESTAMP}.tar.gz"

cleanup() {
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT INT TERM

if [ ! -d "$MATRIX_STORE_DIR" ]; then
  echo "Matrix store directory does not exist: $MATRIX_STORE_DIR" >&2
  exit 1
fi

found=0
find "$MATRIX_STORE_DIR" -type f -name 'matrix-sdk-*.sqlite3' -print | while IFS= read -r source; do
  relative=${source#"$MATRIX_STORE_DIR"/}
  destination="$WORK_DIR/$relative"
  mkdir -p "$(dirname "$destination")"
  sqlite3 -readonly "$source" ".backup '$destination'"
  found=1
done

# `found` is set in a pipeline subshell, so validate the work tree instead.
if ! find "$WORK_DIR" -type f -name 'matrix-sdk-*.sqlite3' -print -quit | grep -q .; then
  echo "No Matrix SDK SQLite databases found in $MATRIX_STORE_DIR; backup skipped." >&2
  exit 1
fi

tar -C "$WORK_DIR" -czf "$ARCHIVE" .
(cd "$BACKUP_DIR" && sha256sum "$(basename "$ARCHIVE")" > "$(basename "$ARCHIVE").sha256")
find "$BACKUP_DIR" -maxdepth 1 -type f \( -name 'matrix_store_backup_*.tar.gz' -o -name 'matrix_store_backup_*.tar.gz.sha256' \) -mtime +"$RETENTION_DAYS" -delete
echo "Matrix SQLite backup successful: $(basename "$ARCHIVE")"
