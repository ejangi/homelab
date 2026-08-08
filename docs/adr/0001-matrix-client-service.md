# Use a dedicated Matrix client service for notifications

Matrix end-to-end encryption requires durable device keys, Olm/Megolm sessions, device tracking, and synchronization, which do not fit safely in ephemeral n8n executions. We will run a Rust Matrix Service backed by the existing Postgres instance; n8n will call it through a small authenticated HTTP API.

## Considered Options

- Pure n8n workflow using HTTP Request and Code nodes.
- A TypeScript Matrix client service.
- A Rust Matrix client service using the Matrix Rust SDK and its Postgres SQL store.

## Consequences

The Matrix Service becomes the sole owner of Matrix credentials and cryptographic state. It adds a container, service-owned database migrations, and a setup interface, while leaving calling workflows free of Matrix authentication and encryption logic. The currently available Postgres adapter is a beta tied to Matrix SDK 0.5; the service isolates its store in the `matrix_sdk` schema and installs a documented idempotent compatibility trigger for that adapter's outbound-session insert defect.
