# AdGuard DNS query log archive

The active **AdGuard DNS query log archive** workflow (`XFLwWZZl0uYNPABS`)
collects the router's DNS query logs every 15 minutes into the **AdGuard DNS
Query Logs** n8n data table (`JkQHRhdOamhupNb1`). Open **Data tables** in n8n
to browse, filter, or export the records for analysis.

Each run deletes records whose query time is more than seven days old, reads
the previously archived queries from the last hour, and pages through
`http://10.0.0.1:3000/control/querylog` in batches of 1,000. Pagination follows
AdGuard's original `oldest` cursor until it covers the one-hour collection
window. The workflow fails if its 200-page safety limit leaves that window
incomplete. New rows are inserted in bulk after comparison with stored IDs.
The ten-minute execution timeout is shorter than the polling interval.
Avoid starting concurrent manual collections.

Identifiers preserve the full nanosecond query timestamp, client, question,
and protocol. The table also includes query time, client address and name,
domain, DNS query type, status, filtering reason, blocked/cache flags, elapsed
milliseconds, upstream, and the complete original query record in `raw_json`.
The `blocked` flag covers AdGuard's `Filtered*` reasons; DNS rewrites are not
classified as blocked. Dates are stored in UTC for sorting and analysis.

The router still retains only one hour. Rereading that window allows recovery
from retries and short outages. Queries that expire from the router before a
successful collection cannot be recovered. On installation, only the existing
hour can be backfilled; seven days accumulate over time. Pruning runs before
collection even when AdGuard is unavailable; if n8n itself is stopped, pruning
resumes when the workflow next runs.

## Credentials, storage, and maintenance

The HTTP Request node uses the encrypted **AdGuard Home router** n8n Basic
Auth credential. The local `.env` contains `ADGUARD_BASE_URL`,
`ADGUARD_USERNAME`, and `ADGUARD_PASSWORD` as working settings. They are not
passed to the container; changing them alone does not change the n8n credential
or the workflow URL. Update those in n8n when changing router settings.
The checked-in workflow contains only a credential reference.

Compose sets the shared data-table allowance to 1 GiB, configurable through
`N8N_DATA_TABLES_MAX_SIZE_BYTES` in `.env`. The initial observed rate of roughly
2,750 queries per hour projects to about 510 MiB of serialized rows per week;
actual database size and traffic will vary. All other n8n data tables share
this allowance. Collection fails visibly if the allowance is exhausted and
uses the existing execution failure notification workflow. The data table
lives in the existing Postgres database and is included in its backups.
Those backups have their own retention, so seven-day pruning applies to the
live table, not historical backup files.

Successful, failed, and manual execution payloads are not retained by this
workflow, avoiding an additional saved copy of DNS history. Development
execution records and the temporary development webhook were removed after
verification. Failure notifications remain enabled through the existing
error workflow.

The workflow definition is in
[`src/adguard/adguard-query-log-archive-workflow.json`](../src/adguard/adguard-query-log-archive-workflow.json).
When importing into another instance, create the table with the columns and
types in the **Store new queries** node's schema, create the Basic Auth
credential, and select those resources in the imported nodes before activation.
The query preparation source is maintained separately in
[`src/adguard/prepare-query-rows.js`](../src/adguard/prepare-query-rows.js);
keep its copy in the workflow JSON synchronized when editing.

Run `node scripts/test-adguard-query-rows.cjs` to check deduplication,
nanosecond timestamp preservation, paging limits, and invalid responses.
Live verification confirmed multi-page collection, repeat collection with no
duplicate IDs, deletion of a seeded eight-day-old record, and an active final
schedule with no development webhook.

API reference: [AdGuard Home OpenAPI](https://github.com/AdguardTeam/AdGuardHome/blob/master/openapi/openapi.yaml).

## Matrix blocked-request notifications

After archiving, the parent calls **AdGuard blocked request notifications**
(`Ubrl1onhTqN2sE5v`), including on runs with no newly archived queries so failed
notifications can be retried. The child sends nothing when no blocked queries
are awaiting delivery. It groups requests by normalized domain and client
address, displaying the client name or its address when unnamed. The HTML table
shows domain, client, and blocked-request count, sorted by descending count.
Reporting timestamps use Australia/Brisbane. Tables split at 40 rows or a
conservative message-size threshold; each part reports its own totals.

The **AdGuard Matrix Notification Batches** data table (`gQtwYFaDvtjVUHHj`)
is an outbox with `batch_id` (string), `message` (string), `query_ids` (string
containing a JSON array), `delivered` (boolean), `matrix_event_id` (string), and
`prepared_at` (date). Query IDs in both pending and completed batches prevent
repeat notifications from the overlapping collection window. Immutable batches
are inserted before delivery; the Matrix Sender uses `batch_id` as its request
ID. A batch is marked delivered only after the sender succeeds. A failure after
Matrix accepts a message therefore retries the same payload and idempotency key.
Delivered batches are pruned after eight days, beyond query retention; pending
batches remain available for retry. The table stores DNS notification content
and is covered by the existing Postgres backups. Do not run concurrent parent
or child executions: the outbox check and insert are not a transactional lock.

Initial notification coverage starts at 30 October 2026, 12:16 pm AEST; older
archived queries are intentionally excluded. Undelivered queries remain eligible
while present in the seven-day archive. Pending prepared batches can outlive
that archive; recover delivery within the Matrix service's 30-day idempotency
retention to preserve duplicate suppression after an ambiguous delivery failure.

The child definition is
[`src/adguard/adguard-blocked-notifications-workflow.json`](../src/adguard/adguard-blocked-notifications-workflow.json),
with grouping source in
[`src/adguard/prepare-blocked-notifications.js`](../src/adguard/prepare-blocked-notifications.js).
Keep the embedded Code node synchronized. On another instance, create the
outbox columns above, select both tables and the Matrix Sender, adjust the
coverage-start filter, publish the child, then select it in and publish the
parent. Successful and failed execution payload retention remains disabled.

Run `node scripts/test-adguard-blocked-notifications.cjs` for grouping, escaping,
address fallback, duplicate suppression, empty runs, and message splitting.
Live verification sent a labelled fictional-client preview through the child,
confirmed a single delivered outbox batch, and reran it without another delivery.
The temporary webhook workflows and seeded query records were removed.
