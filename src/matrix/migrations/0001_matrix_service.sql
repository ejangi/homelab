CREATE SCHEMA IF NOT EXISTS matrix_service;

CREATE TABLE IF NOT EXISTS matrix_service.client_state (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    device_id TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS matrix_service.idempotency_records (
    request_key TEXT PRIMARY KEY,
    request_hash TEXT NOT NULL,
    room_id TEXT NOT NULL,
    encrypted BOOLEAN NOT NULL,
    event_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('in_progress', 'complete')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idempotency_records_completed_at_idx
    ON matrix_service.idempotency_records (completed_at)
    WHERE status = 'complete';
