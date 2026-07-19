CREATE TABLE agent_run_budget_telemetry (
    run_id TEXT PRIMARY KEY NOT NULL,
    manifest_sha256 TEXT NOT NULL CHECK (
        length(manifest_sha256) = 64
        AND manifest_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    capability_schema_version INTEGER NOT NULL CHECK (capability_schema_version > 0),
    telemetry_schema_version INTEGER NOT NULL CHECK (telemetry_schema_version > 0),
    telemetry_json BLOB NOT NULL CHECK (length(telemetry_json) > 0),
    recorded_at_ms INTEGER NOT NULL CHECK (recorded_at_ms >= 0),
    FOREIGN KEY (run_id) REFERENCES agent_run_results (run_id)
        ON UPDATE RESTRICT ON DELETE CASCADE,
    FOREIGN KEY (run_id, manifest_sha256)
        REFERENCES agent_runs (run_id, manifest_sha256)
        ON UPDATE RESTRICT ON DELETE CASCADE
) STRICT;

UPDATE schema_metadata
SET value = '6', updated_at_ms = 0
WHERE key = 'application_schema_version';
