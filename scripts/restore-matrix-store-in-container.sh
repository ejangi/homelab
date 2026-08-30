#!/bin/sh

# Restore a verified Matrix SQLite archive into the persistent store. This is
# intentionally run only by restore-matrix-store.sh while Matrix is stopped.
set -eu

MATRIX_STORE_DIR=${MATRIX_STORE_DIR:-/matrix-store}
BACKUP_DIR=${BACKUP_DIR:-/backups}
ARCHIVE=${MATRIX_STORE_BACKUP_ARCHIVE:?MATRIX_STORE_BACKUP_ARCHIVE is required}
CHECKSUM_FILE="${ARCHIVE}.sha256"

case "$ARCHIVE" in
  "$BACKUP_DIR"/matrix_store_backup_*.tar.gz) ;;
  *)
    echo "Refusing archive outside the Matrix backup directory: $ARCHIVE" >&2
    exit 1
    ;;
esac

if [ ! -f "$ARCHIVE" ] || [ ! -f "$CHECKSUM_FILE" ]; then
  echo "Archive or checksum file is missing." >&2
  exit 1
fi

(cd "$(dirname "$ARCHIVE")" && sha256sum -c "$(basename "$CHECKSUM_FILE")")

# Reject absolute paths and path traversal before extracting anything.
if tar -tzf "$ARCHIVE" | grep -Eq '(^/|(^|/)\.\.(/|$))'; then
  echo "Archive contains an unsafe path." >&2
  exit 1
fi

STAGING_DIR=$(mktemp -d /tmp/matrix-store-restore.XXXXXX)
cleanup() {
  rm -rf "$STAGING_DIR"
}
trap cleanup EXIT INT TERM

tar -xzf "$ARCHIVE" -C "$STAGING_DIR"
if ! find "$STAGING_DIR" -type f -name 'matrix-sdk-*.sqlite3' -print -quit | grep -q .; then
  echo "Archive contains no Matrix SDK SQLite database." >&2
  exit 1
fi

# The caller has already stopped Matrix. Replace only after the archive has
# passed checksum, path, and content validation.
find "$MATRIX_STORE_DIR" -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +
cp -a "$STAGING_DIR"/. "$MATRIX_STORE_DIR"/
echo "Matrix SQLite store restored from $(basename "$ARCHIVE")"
