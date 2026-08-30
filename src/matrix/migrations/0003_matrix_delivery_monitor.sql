CREATE SCHEMA IF NOT EXISTS matrix_monitor;

ALTER TABLE matrix_service.client_state
    ADD COLUMN IF NOT EXISTS monitor_device_id TEXT;

CREATE TABLE IF NOT EXISTS matrix_service.monitor_receipts (
    event_id TEXT PRIMARY KEY,
    room_id TEXT NOT NULL,
    sender TEXT NOT NULL,
    decrypted_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS monitor_receipts_decrypted_at_idx
    ON matrix_service.monitor_receipts (decrypted_at);
