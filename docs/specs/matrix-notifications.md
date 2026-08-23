# Matrix notifications service and workflow specification

## Purpose

Provide one reusable, authenticated way for n8n workflows to post Matrix notifications. The Matrix Service owns Matrix authentication and all client-side encryption state; callers provide only message content and delivery options.

## Scope

The service supports one Matrix account, text messages, Markdown, HTML, optional plaintext delivery, idempotent retries, and Matrix end-to-end encryption for any room that the configured Matrix account has joined and may post to. It can also attach a resized public product or book-cover image.

It does not support arbitrary attachments, reactions, replies, inbound Matrix event processing, room creation, or arbitrary Matrix event types.

## Components

| Component | Responsibility |
| --- | --- |
| `matrix` service | Matrix login, device lifecycle, sync, E2EE, formatting, delivery, persistence, setup, and API-key authentication. |
| Postgres | Matrix SDK state and crypto store, service migrations, delivery idempotency records, and audit metadata. |
| Matrix Sender Workflow | n8n sub-workflow that exposes a typed caller contract and invokes the service. |
| Caddy | Proxies the protected setup UI at `/matrix/`; it does not expose the container port directly. |

## Architecture decisions

- Implement the service in Rust using Rocket and the Matrix Rust SDK.
- Use the Matrix Rust SDK with its compatible `matrix-sdk-sql` Postgres store for Matrix state and encrypted crypto state. Do not implement Olm, Megolm, or Matrix device-key handling directly. The available Postgres-store integration currently pins Matrix SDK 0.5; track its maintenance and upgrade it as a single compatibility unit.
- The service runs continuously so it can synchronize room and device state.
- The service listens on port `8787` inside the Compose network. n8n reaches it at `http://matrix:8787`.
- Do not publish port `8787` directly on the host. Caddy may proxy `/matrix/` to make setup available from the local n8n endpoint and Tailnet hostname.
- The service is the only component that reads Matrix credentials or crypto-store secrets.
- All non-health endpoints require `Authorization: Bearer <MATRIX_SERVICE_API_KEY>`. Do not accept the key in URLs.

## Configuration

The Compose environment provides these values to the `matrix` service. n8n receives only the service URL and API key.

| Variable | Required | Meaning |
| --- | --- | --- |
| `MATRIX_HOMESERVER_URL` | Yes | Matrix homeserver base URL; initially `https://matrix.org`. |
| `MATRIX_USER_ID` | Yes | Service account user ID; initially `@ejangi-integrations:matrix.org`. |
| `MATRIX_PASSWORD` | Yes | Matrix account password. It must never be committed, logged, returned, or included in workflow JSON. |
| `MATRIX_DEFAULT_ROOM_ID` | Yes | Fallback room ID: `!gGNQxnBRzxaGuIcEzJ:matrix.org`. |
| `MATRIX_SERVICE_API_KEY` | Yes | Shared internal API key for n8n and the service. |
| `MATRIX_STORE_ENCRYPTION_KEY` | Yes | Separate secret used to encrypt the Matrix crypto store at rest. |
| `MATRIX_DATABASE_URL` | Yes | Postgres connection URL for the service. |
| `MATRIX_IDEMPOTENCY_RETENTION_DAYS` | No | Retention period for completed delivery records; default `30`. |
| `MATRIX_SERVICE_URL` | n8n only | Internal service base URL; initially `http://matrix:8787`. |

The operator backs up `.env` and its secrets separately. A database backup alone is insufficient to decrypt crypto state without `MATRIX_STORE_ENCRYPTION_KEY`.

## Lifecycle and setup

### Bootstrap

At service startup, the service logs in using the configured account, restores the persistent Matrix client device where possible, initializes the crypto store, uploads any required device keys, and starts synchronization. Startup gives this bootstrap 30 seconds so the health endpoint remains available during an upstream Matrix outage; `POST /v1/setup/bootstrap` retries the operation if startup bootstrap did not complete.

If both the service and SDK state are absent (for example, a new deployment without a restored database), bootstrap creates a new Matrix device. The resulting device cannot decrypt history encrypted for a previously lost device.

### Encryption enablement

The service must never enable encryption as a side effect of a delivery request.

`POST /v1/setup/rooms/{room_id}/enable-encryption` requires an explicit confirmation payload. It checks that the service account is joined to the room and has sufficient power to send the room encryption state event, then enables Matrix E2EE for that room.

For an encrypted delivery request targeting an unencrypted room, the service returns `ROOM_ENCRYPTION_REQUIRED`. The caller or operator must explicitly enable the room first. Plaintext delivery remains available only when the caller chooses it explicitly.

### Verification

The protected setup UI is available at `/matrix/setup` and contains the explicit encryption-enable action. Device verification is not a delivery prerequisite: encrypted messages are sent using the configured SDK policy, which allows unverified devices. A user-facing SAS/QR verification ceremony is deferred from this first release because the delivery policy deliberately does not require verified devices.

## Delivery API

### `POST /v1/messages`

Posts one Matrix text event and, when an image is supplied, an `m.image` event immediately before it.

Request body:

```json
{
  "message": "Deployment is **complete**: https://example.com",
  "room_id": "!optional-room:matrix.org",
  "format": "markdown",
  "encrypted": true,
  "request_id": "deploy-2026-08-08-42",
  "image_url": "https://cdn.shopify.com/example-product.jpg",
  "image_alt": "Example product"
}
```

Rules:

- `message` is required and must be non-empty after validation.
- `room_id` is optional; omitted means `MATRIX_DEFAULT_ROOM_ID`.
- The configured Matrix account must already be joined to the selected room and allowed to send messages there.
- Any joined room may be used. There is no room allowlist in the first release.
- `format` is `text`, `markdown`, or `html`; its default is `markdown`.
- `encrypted` defaults to `true`.
- `encrypted: true` requires that the room has Matrix encryption enabled. Failure to establish or share encryption state fails the request; no plaintext fallback is permitted.
- `encrypted: false` sends a standard plaintext `m.room.message`, even if the room is encrypted. This must be explicit and is recorded in the response and audit metadata.
- `request_id` is optional. When supplied, it is an idempotency key scoped to the effective room and request payload.
- `image_url` is optional. It must be an HTTPS URL on an approved public image CDN (`cdn.shopify.com` for Rushfaster product images, `images.puma.com` for PUMA product images, `res.cloudinary.com` for Proton Blog images, or `i.gr-assets.com` for Goodreads book covers); the service downloads at most 10 MiB, resizes it to fit within 256 × 256 pixels while preserving aspect ratio, encodes it as JPEG, uploads it to Matrix, and sends an `m.image` event before the text message. Redirects are not followed.
- `image_alt` is optional alt text for the image event; it defaults to `Image attachment`.
- The image event is encrypted as a Matrix event when `encrypted: true`, but this initial implementation uploads the JPEG to Matrix without per-attachment encryption because the pinned Matrix SDK 0.5 lacks encrypted media upload support. This is appropriate only for public imagery such as retailer product photos.

Successful response:

```json
{
  "event_id": "$matrix-event-id",
  "image_event_id": "$matrix-image-event-id",
  "room_id": "!gGNQxnBRzxaGuIcEzJ:matrix.org",
  "encrypted": true,
  "idempotent_replay": false,
  "excluded_device_count": 0
}
```

Failure response:

```json
{
  "error": {
    "code": "ROOM_ENCRYPTION_REQUIRED",
    "message": "The room is not configured for end-to-end encryption."
  }
}
```

The API returns stable machine-readable errors for invalid input, unauthorized API access, missing setup, unencrypted-room refusal, and idempotency conflicts. Other Matrix or transport failures are returned as `MATRIX_DELIVERY_FAILED` without exposing internal details.

## Formatting

- `text` produces `m.text` with the supplied value as `body`.
- `markdown` is the default. The service converts Markdown to sanitized Matrix HTML and produces both a plain-text `body` and `formatted_body` with `format: "org.matrix.custom.html"`.
- `html` accepts HTML, sanitizes it, derives a readable plain-text `body`, and sends the sanitized HTML as `formatted_body`.
- The formatter must not fetch URLs or generate previews. URL previews are a Matrix-client capability after message delivery.

## Encryption behavior

For encrypted messages, the Matrix SDK manages device queries, Olm sessions, Megolm sessions, key sharing, rotation, and `m.room.encrypted` delivery. The service must:

- synchronize Matrix state before or while sending;
- share room keys with eligible room devices, including unverified devices, using the Matrix SDK's device policy;
- leave blocked-device handling to the SDK's persisted device-trust state rather than maintaining a second service-side block list;
- return `excluded_device_count: 0` in the initial release because the SDK does not expose a reliable per-send exclusion count;
- serialize crypto mutations so concurrent delivery requests cannot corrupt Matrix crypto state;
- fail the delivery request if no eligible recipient devices can receive the room key.

## Idempotency and cleanup

When `request_id` is present, the service derives a stable Matrix transaction ID and persists the final delivery outcome. Retrying the same effective request returns the original response without creating another Matrix event.

Reusing a `request_id` with different effective content, room, format, or encryption mode returns `IDEMPOTENCY_CONFLICT`.

A scheduled cleanup removes completed idempotency records older than `MATRIX_IDEMPOTENCY_RETENTION_DAYS` (30 days by default). In-progress records are not removed by this job.

## Persistence and migrations

The service owns its tables and migrations. It must:

- use a dedicated Postgres schema or consistently prefixed table names;
- run versioned, forward-only migrations at startup under a Postgres advisory lock;
- keep Matrix SDK storage and custom application storage in the existing Postgres database;
- isolate the Matrix SDK SQL store in the `matrix_sdk` schema so its SQLx migration history cannot collide with the service's `matrix_service` migrations; install the idempotent outbound-session compatibility trigger required by the pinned beta adapter;
- persist only operational metadata such as request hashes, event IDs, timestamps, error codes, and device-exclusion counts;
- never persist Matrix passwords, API keys, or plaintext notification bodies in audit/idempotency tables;
- use the Matrix SDK store-encryption mechanism with `MATRIX_STORE_ENCRYPTION_KEY` for private crypto material.

Existing Postgres backups must include all Matrix Service schemas and tables. Restore documentation must state that `MATRIX_STORE_ENCRYPTION_KEY` is also required.

## n8n Matrix Sender Workflow

The workflow begins with an Execute Sub-workflow Trigger and defines these typed inputs:

| Input | Type | Required | Default |
| --- | --- | --- | --- |
| `message` | string | Yes | — |
| `room_id` | string | No | Service default room |
| `format` | string | No | `markdown` |
| `encrypted` | boolean | No | `true` |
| `request_id` | string | No | — |
| `image_url` | string | No | — |
| `image_alt` | string | No | — |

The imported workflow is stored in `src/matrix/n8n-matrix-sender-workflow.json`. It calls `POST {{$env.MATRIX_SERVICE_URL}}/v1/messages` using an environment-backed `Authorization: Bearer {{$env.MATRIX_SERVICE_API_KEY}}` header. It returns the service response to the calling workflow and fails the execution on non-success responses.

The sub-workflow must not store the Matrix password, access token, device ID, crypto keys, or plaintext message body in static workflow configuration.

## Security requirements

- Constant-time comparison for API keys.
- Health endpoint may be unauthenticated but must disclose no service configuration or Matrix state.
- Redact credentials, access tokens, API keys, device secrets, room keys, and message content from logs.
- Do not include secrets in Compose command lines, image layers, source files, migrations, test fixtures, workflow JSON, or `AGENTS.md`.
- Restrict the Caddy setup route to the existing local/Tailnet n8n endpoint and require the Matrix Service API key for actions.
- Document the service, its configuration names, and the n8n caller contract in `AGENTS.md` during implementation.

## Acceptance criteria

1. `docker compose up` starts the Matrix Service and it passes a health check without publishing its container port directly.
2. Migrations create/upgrade Matrix Service persistence safely and repeatably.
3. An authenticated setup action can initialize the configured Matrix account device.
4. An explicitly confirmed setup action enables encryption on the default room.
5. An n8n workflow can call the reusable Matrix Sender Workflow with only message content and receive a Matrix event ID.
6. Markdown delivery produces a readable plain-text body and sanitized Matrix HTML.
7. Encrypted delivery fails for an unencrypted room until setup explicitly enables encryption; it never silently sends plaintext.
8. An explicit plaintext request sends plaintext and reports `encrypted: false`.
9. Repeating a request with the same `request_id` returns the original Matrix event ID and creates no duplicate event.
10. The service shares encrypted room keys with non-blocked devices regardless of verification status.
11. After a restart, the service restores its persisted Matrix device and crypto state automatically; an operator can retry bootstrap through the protected setup API when startup bootstrap fails.
12. Postgres backup and restore, paired with the separately backed-up store encryption key, restore the service's operational state.
