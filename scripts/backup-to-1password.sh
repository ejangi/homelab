#!/bin/sh

# Upload the newest PostgreSQL dump to the n8n 1Password item and rotate its
# PostgreSQL dump attachments so that only that dump remains in the item.
#
# Example crontab entry (run after the PostgreSQL backup container has had time
# to create its daily dump):
#   15 22 * * * ~/Sites/n8n/scripts/backup-to-1password.sh >> ~/Sites/n8n/backup-to-1password.log 2>&1

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
BACKUP_DIR=${BACKUP_DIR:-"$SCRIPT_DIR/../backups"}
OP_ITEM=${OP_ITEM:-'n8n user (localhost)'}
OP_SECTION=${OP_SECTION:-Headings}

# Set OP_VAULT when the item is not in the default vault. The value is kept out
# of the command when unset, which also works with personal accounts.
op_item_edit() {
	if [ -n "${OP_VAULT:-}" ]; then
		op item edit "$OP_ITEM" --vault "$OP_VAULT" "$@"
	else
		op item edit "$OP_ITEM" "$@"
	fi
}

escape_assignment_name() {
	# Assignment names use backslash to escape syntax-significant characters.
	printf '%s' "$1" | sed 's/[\\.=]/\\&/g'
}

if ! command -v op >/dev/null 2>&1; then
	echo "1Password CLI (op) is not installed or is not on PATH." >&2
	exit 1
fi

if ! command -v jq >/dev/null 2>&1; then
	echo "jq is required to inspect 1Password attachment metadata." >&2
	exit 1
fi

if [ ! -d "$BACKUP_DIR" ]; then
	echo "Backup directory does not exist: $BACKUP_DIR" >&2
	exit 1
fi

# Sort by modification time rather than relying solely on the filename. This
# avoids selecting a partially copied or restored file by accident.
LATEST=$(find "$BACKUP_DIR" -maxdepth 1 -type f -name 'n8n_backup_*.dump' -printf '%T@ %p\n' \
	| sort -n \
	| tail -n 1 \
	| cut -d ' ' -f 2-)

if [ -z "$LATEST" ] || [ ! -f "$LATEST" ]; then
	echo "No n8n PostgreSQL backup dump found in $BACKUP_DIR." >&2
	exit 1
fi

LATEST_NAME=$(basename -- "$LATEST")
LATEST_FIELD=$(escape_assignment_name "$LATEST_NAME")

echo "Uploading $LATEST_NAME to 1Password item '$OP_ITEM'..."
op_item_edit "$OP_SECTION.$LATEST_FIELD[file]=$LATEST" >/dev/null

# Fetch attachment metadata only after the upload succeeds. Avoid --reveal:
# attachment names and section metadata are sufficient and no item secrets are
# written to the temporary file.
ITEM_JSON=$(mktemp)
trap 'rm -f "$ITEM_JSON"' EXIT INT TERM

if [ -n "${OP_VAULT:-}" ]; then
	op item get "$OP_ITEM" --vault "$OP_VAULT" --format=json >"$ITEM_JSON"
else
	op item get "$OP_ITEM" --format=json >"$ITEM_JSON"
fi

# Remove every other PostgreSQL dump attachment in the item. This also cleans
# up attachments left in a different section by an earlier version of this
# script. Keep only the newly uploaded file in the target section; section
# matching is case-insensitive because the UI may display section labels with
# different capitalization.
jq -r --arg section "$OP_SECTION" --arg keep "$LATEST_NAME" '
	.files[]?
	| select((.name // "") | test("^n8n_backup_.*\\.dump$"))
	| select(
		(.name != $keep)
		or ((.section.label // .section.name // "" | ascii_downcase) != ($section | ascii_downcase))
	)
	| [(.section.label // .section.name // ""), .name] | @tsv
' "$ITEM_JSON" | while IFS="$(printf '\t')" read -r old_section old_name; do
	[ -n "$old_section" ] && [ -n "$old_name" ] || continue
	old_field=$(escape_assignment_name "$old_name")
	old_section_field=$(escape_assignment_name "$old_section")
	echo "Removing old 1Password attachment from $old_section: $old_name"
	op_item_edit "$old_section_field.$old_field[delete]" >/dev/null
done

echo "1Password backup rotation complete: $LATEST_NAME"
