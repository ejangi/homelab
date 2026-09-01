ALTER TABLE matrix_service.idempotency_records
    ADD COLUMN IF NOT EXISTS monitor_verified BOOLEAN;

-- Existing encrypted records only reached completion after the monitor receipt
-- was observed. Plaintext records have no monitor verification result.
UPDATE matrix_service.idempotency_records
    SET monitor_verified = TRUE
    WHERE encrypted = TRUE
      AND status = 'complete'
      AND monitor_verified IS NULL;
