# n8n Local Homelab

> Uses [n8n](https://github.com/n8n-io/n8n), serves using https via [caddy](https://github.com/caddyserver/caddy) and do 24 hour backups.

## Installation

Once you clone this repo, ensure you DON'T track the changes to your `.env` file:

```bash
git update-index --assume-unchanged .env
```

Then, edit the `.env` file with the credentials you intend to use.

## Matrix notifications

This Compose environment includes a `matrix` service that lets n8n workflows
send authenticated Matrix notifications, including end-to-end encrypted
messages. The reusable **Matrix Sender** sub-workflow calls it internally.

See the [Matrix notifications service and workflow specification](docs/specs/matrix-notifications.md)
for setup, configuration, caller inputs, encryption behaviour, and recovery
requirements.
