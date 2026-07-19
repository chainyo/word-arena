ALTER TABLE capabilities
ADD COLUMN agent_run_id TEXT REFERENCES agent_runs (run_id) ON DELETE RESTRICT;

CREATE INDEX capabilities_agent_run_idx
    ON capabilities (agent_run_id, issued_at_ms, capability_id)
    WHERE agent_run_id IS NOT NULL;

UPDATE schema_metadata
SET value = '3', updated_at_ms = 0
WHERE key = 'application_schema_version';
