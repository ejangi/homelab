# Container update, 1 November 2026

Pre-update recovery files are in `backups/pre-upgrade-20261101/` (private, local,
not committed). `database-final.dump` is the stopped-writer PostgreSQL dump;
`volumes.tar.gz` contains n8n files, Matrix device state, Grafana, Loki, Alloy,
and Caddy data. `config.tar.gz` includes secrets and must remain private.
Router logs are in `router-logs.tar.gz`. SHA256SUMS verifies archive integrity.
The database dump was restored into a temporary database and its n8n workflow
and credential counts matched production (16 workflows, 1 credential).

All existing named volumes and bind paths are retained. PostgreSQL stays on
major version 15. Never use `docker compose down -v` during updates.
SFTPGo metadata lives in PostgreSQL; SFTPGo and Samba share the existing host
`iCloud Drive` directory (307 GB). That directory was not duplicated locally
because available disk space is insufficient; its bind mount remains unchanged.
Browserless and GNOME notifier have no durable application state.

Both daily backup services previously had restart disabled and were stopped.
Their restart policy is now `unless-stopped`.

For rollback, stop writers first. Restore PostgreSQL and volume snapshots as a
matched set before starting old application images: migrations may prevent
simply reverting image tags. Matrix device state also requires the original
store encryption key from the private environment backup. Restore through the
existing Compose restore scripts where applicable. Preserve file ownership.

Local builds refresh Debian/Alpine packages without changing application
sources or Rust dependency locks. This update is not a comprehensive dependency
vulnerability scan.

Deployment completed with n8n 2.45.7, PostgreSQL 15.19, Caddy 2.11.7,
Browserless 2.58.1 and SFTPGo 2.7.6. n8n readiness, Grafana database health,
Loki readiness, Matrix and GNOME health checks passed. Samba's SMB3 share
listing succeeded. Daily PostgreSQL and Matrix backup jobs produced backups.
Post-update n8n workflow and credential counts remain 16 and 1.
