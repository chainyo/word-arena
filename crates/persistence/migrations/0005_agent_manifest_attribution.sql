CREATE UNIQUE INDEX agent_runs_result_identity_idx
    ON agent_runs (run_id, manifest_sha256);

CREATE UNIQUE INDEX agent_runs_replay_identity_idx
    ON agent_runs (run_id, game_id, seat_number, manifest_sha256);

CREATE TABLE agent_run_results (
    run_id TEXT PRIMARY KEY NOT NULL,
    manifest_sha256 TEXT NOT NULL CHECK (
        length(manifest_sha256) = 64
        AND manifest_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    result_schema_version INTEGER NOT NULL CHECK (result_schema_version > 0),
    outcome_kind TEXT NOT NULL CHECK (
        outcome_kind IN ('finished', 'failed', 'cancelled')
    ),
    completed_at_ms INTEGER NOT NULL CHECK (completed_at_ms >= 0),
    FOREIGN KEY (run_id, manifest_sha256)
        REFERENCES agent_runs (run_id, manifest_sha256)
        ON UPDATE RESTRICT ON DELETE CASCADE
) STRICT;

CREATE TABLE game_replay_agents (
    game_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version >= 0),
    seat_number INTEGER NOT NULL CHECK (seat_number IN (1, 2)),
    run_id TEXT NOT NULL,
    manifest_sha256 TEXT NOT NULL CHECK (
        length(manifest_sha256) = 64
        AND manifest_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    PRIMARY KEY (game_id, version, seat_number),
    UNIQUE (run_id, game_id, version),
    FOREIGN KEY (game_id, version)
        REFERENCES game_replays (game_id, version)
        ON UPDATE RESTRICT ON DELETE CASCADE,
    FOREIGN KEY (run_id, game_id, seat_number, manifest_sha256)
        REFERENCES agent_runs (run_id, game_id, seat_number, manifest_sha256)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT;

CREATE INDEX game_replay_agents_manifest_idx
    ON game_replay_agents (manifest_sha256, game_id, version, seat_number);

UPDATE schema_metadata
SET value = '5', updated_at_ms = 0
WHERE key = 'application_schema_version';
