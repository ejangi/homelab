# Immich

Immich is defined in the repository-root `compose.yml`, with its configuration
in the repository-root `.env` under `IMMICH_`-prefixed variables. All
Immich-managed data is kept on the encrypted volume;
`/mnt/Photos/library` is mounted as `/external-library` with read-only
access, so scans and accidental UI deletions cannot alter originals.

It is available only over Tailscale at:

`https://ejangi-nix.bobtail-morpho.ts.net`

Immich cannot be served from `/photos`; it requires the root path of a host or
subdomain. Caddy serves the root path with a Tailscale-issued, publicly trusted
certificate and rejects non-Tailscale clients. The existing `/n8n`, `/matrix`,
and `/files` routes remain unchanged.

After first sign-in, create an **External Library**, set its import path to
`/external-library`, then run **Scan New Library Files**. Keep it read-only
unless you intentionally want Immich to modify originals or write XMP sidecars.

## Operations

The enabled user service manages the containers. It checks that `/mnt/Photos`
is a real mount point at every boot; if the LUKS volume is still locked, it
retries once per minute without ever starting Immich against the root disk.

```sh
systemctl --user status immich
systemctl --user restart immich
systemctl --user stop immich
```

For upgrades, from the repository root:

```sh
systemctl --user stop immich
docker compose ps
docker compose pull
systemctl --user start immich
```

The Compose bind mounts are configured not to be auto-created on deployment.

## Database backups

The existing `backups/` directory now contains independent daily dumps named
`n8n_backup_<timestamp>.dump` and `immich_backup_<timestamp>.dump`. Both retain
the existing 30-day cleanup policy.

## AMD GPU acceleration

ROCm is not currently enabled. The ROCm container image required more system
disk space than was available during extraction, so the failed image was
removed and Immich continues to use CPU machine learning. Video transcoding is
unchanged.
