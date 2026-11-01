# Orbi syslog receiver

The root Compose `syslog-ng` service receives newline-delimited traditional
syslog over **TCP 5514**, bound only to `SYSLOG_LAN_IP` in the root `.env`.
Current address: **10.0.0.25** on `wlo1`. This address is DHCP-assigned;
reserve it on the router before relying on it. No UDP or WAN forwarding is needed.
Host networking avoids Docker published-port rules bypassing UFW.

Allow the two senders on the host (assuming UFW has a default deny incoming policy):

```sh
sudo ufw allow in on wlo1 proto tcp from 10.0.0.1 to 10.0.0.25 port 5514 comment 'Orbi RBR20 syslog'
sudo ufw allow in on wlo1 proto tcp from 10.0.0.12 to 10.0.0.25 port 5514 comment 'Orbi RBS20 syslog'
sudo ufw status verbose
```

No firewall rules are changed by Compose. Review existing broad allow rules;
these narrow rules do not override existing allows. syslog-ng additionally filters
stored messages by source IP. TCP is unencrypted, intended for this trusted LAN.

Logs are in `logs/orbi/orbi-rbr20.log` and `logs/orbi/orbi-rbs20.log`.
Each line preserves the raw incoming message (including its original timestamp),
parsed hostname, facility/severity, sender IP, parsed sender time, and receipt time.
Times without a supplied offset are interpreted in Australia/Brisbane. Legacy
syslog timestamps lack a year and timezone; receipt time supplies context.
Device separation uses source IP rather than trusting the advertised hostname.
Routine `crond` launch notices for the RBR20's `orbi-agh-sync` and
`orbi-discovery-snapshot` commands are narrowly normalized from `cron.err` to
`cron.info` before storage. The rule requires hostname `orbi-rbr20`, program
`crond`, original facility/severity `cron.err`, and the exact expected command
and redirection. syslog-ng parses the `crond[PID]:` prefix separately from the
message body. All unmatched messages retain their original severity.
The stored `raw=` message retains its original PRI (`<75>`); only the record's
`severity=` changes. Alloy derives Loki's severity label from this normalized
field; Loki derives its detected level from that label. Historical entries are
not rewritten. These launch notices do not establish script success.
Run `python3 scripts/check-orbi-cron-normalization.py` to exercise the real
syslog-ng parser/rewrite with positive and negative fixtures in isolation.
All message types are retained, including authentication, Wi-Fi, and RBR20
`ORBI_WAN_INPUT ` / `ORBI_WAN_FORWARD ` events; no extra receiver rate limit applies.

Rotation checks run every minute: daily or above 20 MiB, with 14 compressed
archives per device and maximum archive age 14 days (pruned during rotation).
This bounds archive count and typical storage to about 300 MiB uncompressed
per device, plus traffic arriving between checks. It is not a hard disk quota.
These logs are not part of the PostgreSQL backup.

```sh
docker compose up -d --build syslog-ng
docker compose logs --tail=30 syslog-ng
sudo grep -F 'orbi-syslog-test' logs/orbi/*.log
sudo zgrep -F 'orbi-syslog-test' logs/orbi/*.gz
```

After forwarding is configured, generate a uniquely labelled `orbi-syslog-test`
message on each router. Confirm each label appears only in its corresponding
file with the expected hostname and source IP. Router-originated delivery cannot
be verified until forwarding and the host firewall rules are configured.

Configuration uses syslog-ng's [source options](https://syslog-ng.github.io/admin-guide/060_Sources/170_Syslog/000_syslog_source_options)
for hostname and raw-message preservation.

## Grafana log search

Grafana is served at `https://ejangi-nix.bobtail-morpho.ts.net/logs/`
through the existing Caddy HTTPS endpoint. Log in as `admin` with the initial
`GRAFANA_ADMIN_PASSWORD` stored only in the root `.env`. Grafana stores subsequent
password changes in its persistent database; changing the bootstrap environment
variable does not reset an existing account.

Open **Explore**, select **Loki**, and choose a time range. Example queries:

```logql
{job="orbi"}
{job="orbi", device="orbi-rbr20"}
{job="orbi"} |= "orbi-syslog-test"
{job="orbi", device="orbi-rbr20"} |~ "ORBI_WAN_(INPUT|FORWARD) "
{job="orbi", severity=~"err|crit|alert|emerg"}
```

Alloy reads the active router files read-only and persists read positions in
`alloy_data`. Device, hostname, facility and severity are indexed labels; full
messages remain searchable. Search timestamps use the receiver timestamp,
including its Brisbane offset. Initial ingestion reads existing active files;
compressed archives are not imported. Rotation continues independently.

Loki uses local filesystem storage with TSDB indexes and compactor-managed
14-day retention. Deletion is asynchronous and is not a disk quota; monitor host
free space. `loki_data` persists logs, `grafana_data` persists users/settings,
and `alloy_data` persists read positions. These volumes are separate from the
PostgreSQL backup and are not currently backed up.

No new firewall ports are needed for the UI. Grafana, Loki and Alloy use the
existing private Compose network; only Caddy exposes the authenticated UI.

```sh
docker compose up -d grafana loki alloy
docker compose exec -T caddy caddy validate --config /etc/caddy/Caddyfile
docker compose exec -T caddy caddy reload --config /etc/caddy/Caddyfile
docker compose logs --tail=30 grafana loki alloy
```

References: [Grafana sub-path hosting](https://grafana.com/tutorials/run-grafana-behind-a-proxy/),
[Alloy file collection](https://grafana.com/docs/alloy/latest/tutorials/send-logs-to-loki/),
and [Loki retention](https://grafana.com/docs/loki/latest/operations/storage/retention/).

A provisioned **Orbi logs** dashboard at `/logs/d/orbi-logs` includes all logs,
WAN firewall events, authentication events, and Wi-Fi events. The last two panels
use text matching and may include unrelated lines containing those terms.
