# Use maintained Matrix SDK SQLite crypto stores

The Matrix Service will use the maintained Matrix Rust SDK SQLite store for
sender and delivery-monitor state. Each client has its own directory in the
`matrix_store` Docker volume and uses a distinct key derived from
`MATRIX_STORE_ENCRYPTION_KEY`. Postgres remains the source of truth for service
metadata, idempotency, monitor receipts, and failure audit records.

## Context

The previous Postgres adapter was an unmaintained beta tied to Matrix Rust SDK
0.5. It could not reliably process modern Matrix protocol events or establish
new Olm sessions, producing messages clients could not decrypt.

## Consequences

This migration intentionally creates new Matrix devices instead of trying to
reuse incompatible cryptographic state. The SDK handles current device-key
queries and room-key rotation. A dedicated backup service uses SQLite's online
backup API to create daily, WAL-consistent archives in `backups/`; the existing
1Password job also retains the newest archive off-host. Restoring a crypto
store is a deliberate whole-service recovery operation and must be done while
the Matrix service is stopped.
