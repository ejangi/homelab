#!/bin/sh

# User services cannot approve an interactive 1Password desktop prompt. A
# service-account token lets the scheduled job run unattended; without one,
# preserve the local verified backup and exit successfully rather than looping
# on failed authorization prompts.
set -eu

if [ -z "${OP_SERVICE_ACCOUNT_TOKEN:-}" ]; then
  echo "Skipping off-host backup: OP_SERVICE_ACCOUNT_TOKEN is not configured."
  exit 0
fi

exec /home/ejangi/Sites/n8n/scripts/backup-to-1password.sh
