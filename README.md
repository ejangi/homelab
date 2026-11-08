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

## Nextcloud iCloud Drive

Open https://ejangi-nix.bobtail-morpho.ts.net/cloud/ with Tailscale connected.
Sign in as `admin` using `NEXTCLOUD_ADMIN_PASSWORD` in the local `.env`.
Caddy uses the existing Tailscale HTTPS certificate.

The existing `ICLOUD_DRIVE_DIR` (default `/home/ejangi/iCloud Drive`) is
mounted at `/mnt/icloud` as local external storage named **iCloud Drive**.
It is separate from Nextcloud's internal `nextcloud_data` volume. Never remove
that host directory when retiring containers. SFTPGo and Samba are retired;
their old database tables remain available for recovery.

Nextcloud uses `oc_` tables in the existing Postgres database, covered by the
existing nightly dump. Also back up the `nextcloud_data` volume (configuration,
apps and internal files) and the external iCloud Drive directory; the database
dump alone cannot restore files. The cron service runs background jobs.

The custom image maps www-data to host UID/GID 1000, preserving existing file
ownership. Rebuild both Nextcloud services together on upgrades. External file
changes are checked on access; a full rescan can be run when needed with
`docker compose exec -u www-data nextcloud php occ files:scan --all`.

## AdGuard DNS query logs

The **AdGuard DNS query log archive** workflow collects router query logs every
15 minutes into the **AdGuard DNS Query Logs** n8n data table and keeps a rolling
seven days. See [setup and maintenance](docs/adguard-query-log-archive.md).

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

## BleepingComputer article alerts

The [src/bleepingcomputer/](src/bleepingcomputer/) workflow reads the
BleepingComputer RSS feed every hour, records each delivered article in the
`BleepingComputer Articles` n8n data table, and sends new articles through
Matrix Sender. Each message links the title to the article and includes its
categories and description.

## Orbi syslog

See [Orbi syslog receiver](docs/orbi-syslog.md) for the LAN TCP endpoint,
UFW rules, retention, router verification commands, and Grafana search at `/logs/`.

THE ICONIC adidas crossbody and messenger alerts: [setup and behavior](docs/iconic-adidas-bags.md).
