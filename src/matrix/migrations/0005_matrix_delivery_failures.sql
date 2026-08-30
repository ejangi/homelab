CREATE TABLE IF NOT EXISTS matrix_service.monitor_delivery_failures (
    event_id TEXT PRIMARY KEY,
    room_id TEXT NOT NULL,
    attempt SMALLINT NOT NULL CHECK (attempt > 0),
    failure_kind TEXT NOT NULL,
    failed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    recovered_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS monitor_delivery_failures_failed_at_idx
    ON matrix_service.monitor_delivery_failures (failed_at DESC);
