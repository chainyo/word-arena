CREATE UNIQUE INDEX matches_telemetry_identity_idx
    ON matches (match_id, tournament_id);

CREATE TABLE agent_run_telemetry (
    run_id TEXT PRIMARY KEY NOT NULL,
    manifest_sha256 TEXT NOT NULL CHECK (
        length(manifest_sha256) = 64
        AND manifest_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    telemetry_schema_version INTEGER NOT NULL CHECK (telemetry_schema_version > 0),
    redaction_policy_version INTEGER NOT NULL CHECK (redaction_policy_version > 0),
    tournament_id TEXT,
    match_id TEXT,
    game_id TEXT NOT NULL,
    seat_number INTEGER NOT NULL CHECK (seat_number IN (1, 2)),
    telemetry_json BLOB NOT NULL CHECK (
        length(telemetry_json) > 0 AND length(telemetry_json) <= 33554432
    ),
    retention_kind TEXT NOT NULL CHECK (retention_kind IN ('retain', 'expire')),
    expires_at_ms INTEGER CHECK (expires_at_ms IS NULL OR expires_at_ms >= 0),
    recorded_at_ms INTEGER NOT NULL CHECK (recorded_at_ms >= 0),
    CHECK (
        (retention_kind = 'retain' AND expires_at_ms IS NULL)
        OR (retention_kind = 'expire' AND expires_at_ms IS NOT NULL)
    ),
    CHECK (tournament_id IS NULL OR match_id IS NOT NULL),
    FOREIGN KEY (tournament_id) REFERENCES tournaments (tournament_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (match_id) REFERENCES matches (match_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (match_id, tournament_id)
        REFERENCES matches (match_id, tournament_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (run_id) REFERENCES agent_run_results (run_id)
        ON UPDATE RESTRICT ON DELETE CASCADE,
    FOREIGN KEY (run_id, game_id, seat_number, manifest_sha256)
        REFERENCES agent_runs (run_id, game_id, seat_number, manifest_sha256)
        ON UPDATE RESTRICT ON DELETE CASCADE
) STRICT;

CREATE INDEX agent_run_telemetry_correlation_idx ON agent_run_telemetry (
    tournament_id, match_id, game_id, run_id
);

CREATE INDEX agent_run_telemetry_expiry_idx ON agent_run_telemetry (
    expires_at_ms, run_id
) WHERE expires_at_ms IS NOT NULL;

UPDATE schema_metadata
SET value = '7', updated_at_ms = 0
WHERE key = 'application_schema_version';
