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

## SFTPGo iCloud Drive

The Compose stack includes [SFTPGo](https://docs.sftpgo.com/) for web-based
access to `/home/ejangi/iCloud Drive`. It is available at
`https://<N8N_HOST>/files`; set `ICLOUD_DRIVE_DIR` in `.env` only if that
directory is elsewhere.

Before its first start, add a strong, unique SFTPGo administrator password to
`.env`:

```bash
SFTPGO_ADMIN_PASSWORD=<generate-a-strong-password>
```

Start it with:

```bash
docker compose up -d sftpgo
```

Sign in at `/files/web/admin`, create a regular SFTPGo user with home directory
`/srv/sftpgo/data`, then use that account at `/files/web/client` to manage the
iCloud Drive files. The SFTP server itself is disabled in this web-only setup.

SFTPGo stores its configuration, users, shares, and audit data in the existing
Postgres database using an `sftpgo_` table prefix. Its data is therefore already
included in the existing nightly PostgreSQL dump; no extra database service or
backup process is needed.

## iPhone Files (SMB)

The stack also exposes the same directory as the authenticated SMB share
`iCloudDrive`. This is separate from the HTTPS web UI: `/files` is only for a
browser, while the iPhone Files app connects to:

```
smb://<N8N_HOST>/iCloudDrive
```

Add a strong, unique password to `.env` before starting the share:

```bash
SMB_PASSWORD=<generate-a-strong-password>
```

Then run:

```bash
docker compose up -d samba
```

In **Files** on iPhone, choose **Browse** → **More** → **Connect to Server**,
enter the SMB address above, choose **Registered User**, and enter username
`files` with the `SMB_PASSWORD` value. Keep the Tailscale VPN connected when
away from the home network. SMB is exposed on TCP port 445 and does not use
Caddy or the `/files` URL.

## DONKI CME alerts

The [src/donki/](src/donki/) directory contains the n8n workflow definition for
hourly alerts about Earth-directed coronal mass ejections (CMEs). It polls a
rolling two-day DONKI window, keeps delivered reports in the `DONKI CME Reports`
n8n data table, and sends human-readable notifications through Matrix Sender.

## Goodreads science-fiction alerts

The [src/goodreads/](src/goodreads/) workflow checks the configured Goodreads
genre API every day, records each delivered book in the `Goodreads Science
Fiction Books` n8n data table, and sends newly listed books through Matrix
Sender with their Goodreads cover image.
