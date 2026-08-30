ALTER TABLE matrix_service.client_state
    ADD COLUMN IF NOT EXISTS monitor_user_id TEXT;
