#!/bin/sh

# The filename includes the current date for easy identification
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
BACKUP_FILE="${BACKUP_DIR}/n8n_backup_${TIMESTAMP}.dump"

# --- 1. Perform Backup ---
echo "Starting PostgreSQL backup for database $PGDATABASE at $TIMESTAMP..."

# Use pg_dump to create a custom-format backup file.
pg_dump -Fc -h $PGHOST -p $PGPORT -U $PGUSER -d $PGDATABASE > "$BACKUP_FILE"

if [ $? -eq 0 ]; then
  echo "Backup successful! File created: $BACKUP_FILE"

  # --- 2. Clean Up Old Backups ---

  # Define the retention period (30 days for approximately one month)
  RETENTION_DAYS=30

  echo "Cleaning up backups older than $RETENTION_DAYS days (1 month)..."

  # The 'find' command searches the backup directory:
  # -type f: looks only for regular files
  # -name '*.dump': looks for files matching the backup format
  # -mtime +$RETENTION_DAYS: finds files whose data was last modified more than $RETENTION_DAYS ago
  # -exec rm {} \;: executes the 'rm' command on each found file
  find $BACKUP_DIR -type f -name 'n8n_backup_*.dump' -mtime +$RETENTION_DAYS -exec rm {} \;

  echo "Cleanup complete. Files older than $RETENTION_DAYS days have been removed."
else
  echo "Backup FAILED!"
fi

echo "Next backup in 24 hours."
