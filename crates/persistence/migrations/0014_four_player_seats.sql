-- no-transaction
PRAGMA foreign_keys = OFF;

ALTER TABLE agent_run_budget_telemetry RENAME TO agent_run_budget_telemetry_seat_v13;
ALTER TABLE agent_run_telemetry RENAME TO agent_run_telemetry_seat_v13;
ALTER TABLE game_replay_agents RENAME TO game_replay_agents_seat_v13;
ALTER TABLE agent_run_results RENAME TO agent_run_results_seat_v13;
ALTER TABLE capabilities RENAME TO capabilities_seat_v13;
ALTER TABLE audit_records RENAME TO audit_records_seat_v13;
ALTER TABLE agent_runs RENAME TO agent_runs_seat_v13;
ALTER TABLE private_events RENAME TO private_events_seat_v13;
ALTER TABLE turn_deadlines RENAME TO turn_deadlines_seat_v13;
ALTER TABLE invalid_attempt_counters RENAME TO invalid_attempt_counters_seat_v13;
ALTER TABLE seats RENAME TO seats_seat_v13;

CREATE TABLE seats (
    game_id TEXT NOT NULL,
    seat_number INTEGER NOT NULL CHECK (seat_number BETWEEN 1 AND 4),
    participant_kind TEXT NOT NULL CHECK (participant_kind IN ('unassigned', 'human', 'agent')),
    participant_id TEXT,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    PRIMARY KEY (game_id, seat_number),
    FOREIGN KEY (game_id) REFERENCES games (game_id) ON DELETE CASCADE,
    CHECK (
        (participant_kind = 'unassigned' AND participant_id IS NULL)
        OR (participant_kind <> 'unassigned' AND participant_id IS NOT NULL)
    )
) STRICT;

CREATE TABLE private_events (
    game_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    seat_number INTEGER NOT NULL CHECK (seat_number BETWEEN 1 AND 4),
    event_schema_version INTEGER NOT NULL CHECK (event_schema_version > 0),
    payload_json BLOB NOT NULL CHECK (length(payload_json) > 0),
    committed_at_ms INTEGER NOT NULL CHECK (committed_at_ms >= 0),
    PRIMARY KEY (game_id, sequence, seat_number),
    FOREIGN KEY (game_id, seat_number) REFERENCES seats (game_id, seat_number)
        ON DELETE CASCADE,
    FOREIGN KEY (game_id, sequence) REFERENCES public_events (game_id, sequence)
        ON DELETE CASCADE
) STRICT;

CREATE TABLE agent_runs (
    run_id TEXT PRIMARY KEY NOT NULL,
    match_id TEXT,
    game_id TEXT NOT NULL,
    seat_number INTEGER NOT NULL CHECK (seat_number BETWEEN 1 AND 4),
    manifest_sha256 TEXT NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN ('pending', 'starting', 'running', 'finished', 'failed', 'cancelled')
    ),
    started_at_ms INTEGER CHECK (started_at_ms IS NULL OR started_at_ms >= 0),
    finished_at_ms INTEGER CHECK (finished_at_ms IS NULL OR finished_at_ms >= 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    UNIQUE (game_id, seat_number),
    FOREIGN KEY (match_id) REFERENCES matches (match_id) ON DELETE SET NULL,
    FOREIGN KEY (game_id, seat_number) REFERENCES seats (game_id, seat_number)
        ON DELETE CASCADE,
    FOREIGN KEY (manifest_sha256) REFERENCES agent_manifests (manifest_sha256)
        ON DELETE RESTRICT,
    CHECK (finished_at_ms IS NULL OR started_at_ms IS NOT NULL),
    CHECK (started_at_ms IS NULL OR started_at_ms >= created_at_ms),
    CHECK (finished_at_ms IS NULL OR finished_at_ms >= started_at_ms)
) STRICT;

CREATE TABLE capabilities (
    capability_id TEXT PRIMARY KEY NOT NULL,
    game_id TEXT NOT NULL,
    seat_number INTEGER CHECK (seat_number BETWEEN 1 AND 4),
    authority_kind TEXT NOT NULL CHECK (
        authority_kind IN ('public', 'seat', 'human_spectator', 'administrator')
    ),
    scopes TEXT NOT NULL CHECK (length(scopes) > 0),
    token_digest BLOB NOT NULL CHECK (length(token_digest) = 32),
    digest_version INTEGER NOT NULL CHECK (digest_version > 0),
    issued_at_ms INTEGER NOT NULL CHECK (issued_at_ms >= 0),
    expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms > issued_at_ms),
    revoked_at_ms INTEGER CHECK (revoked_at_ms IS NULL OR revoked_at_ms >= issued_at_ms),
    agent_run_id TEXT REFERENCES agent_runs (run_id) ON DELETE RESTRICT,
    UNIQUE (token_digest),
    FOREIGN KEY (game_id) REFERENCES games (game_id) ON DELETE CASCADE,
    FOREIGN KEY (game_id, seat_number) REFERENCES seats (game_id, seat_number)
        ON DELETE CASCADE,
    CHECK (
        (authority_kind = 'seat' AND seat_number IS NOT NULL)
        OR (authority_kind <> 'seat' AND seat_number IS NULL)
    )
) STRICT;

CREATE TABLE audit_records (
    audit_id INTEGER PRIMARY KEY AUTOINCREMENT,
    game_id TEXT,
    actor_kind TEXT NOT NULL CHECK (
        actor_kind IN ('public', 'seat', 'human_spectator', 'administrator', 'system')
    ),
    seat_number INTEGER CHECK (seat_number BETWEEN 1 AND 4),
    action TEXT NOT NULL CHECK (length(action) > 0),
    outcome TEXT NOT NULL CHECK (length(outcome) > 0),
    metadata_json BLOB CHECK (metadata_json IS NULL OR length(metadata_json) > 0),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0),
    FOREIGN KEY (game_id) REFERENCES games (game_id) ON DELETE RESTRICT,
    FOREIGN KEY (game_id, seat_number) REFERENCES seats (game_id, seat_number)
        ON DELETE RESTRICT,
    CHECK (
        (actor_kind = 'seat' AND game_id IS NOT NULL AND seat_number IS NOT NULL)
        OR (actor_kind <> 'seat' AND seat_number IS NULL)
    )
) STRICT;

CREATE TABLE turn_deadlines (
    game_id TEXT NOT NULL,
    turn_number INTEGER NOT NULL CHECK (turn_number >= 0),
    seat_number INTEGER NOT NULL CHECK (seat_number BETWEEN 1 AND 4),
    deadline_at_ms INTEGER NOT NULL CHECK (deadline_at_ms >= 0),
    policy_version INTEGER NOT NULL CHECK (policy_version > 0),
    PRIMARY KEY (game_id, turn_number),
    FOREIGN KEY (game_id) REFERENCES games (game_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE invalid_attempt_counters (
    game_id TEXT NOT NULL,
    turn_number INTEGER NOT NULL CHECK (turn_number >= 0),
    seat_number INTEGER NOT NULL CHECK (seat_number BETWEEN 1 AND 4),
    policy_version INTEGER NOT NULL CHECK (policy_version > 0),
    attempt_count INTEGER NOT NULL CHECK (attempt_count > 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0),
    PRIMARY KEY (game_id, turn_number, seat_number),
    FOREIGN KEY (game_id) REFERENCES games (game_id) ON DELETE CASCADE
) STRICT;

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
    seat_number INTEGER NOT NULL CHECK (seat_number BETWEEN 1 AND 4),
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
    seat_number INTEGER NOT NULL CHECK (seat_number BETWEEN 1 AND 4),
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

INSERT INTO seats SELECT * FROM seats_seat_v13;
INSERT INTO private_events SELECT * FROM private_events_seat_v13;
INSERT INTO agent_runs SELECT * FROM agent_runs_seat_v13;
INSERT INTO capabilities SELECT * FROM capabilities_seat_v13;
INSERT INTO audit_records SELECT * FROM audit_records_seat_v13;
INSERT INTO turn_deadlines SELECT * FROM turn_deadlines_seat_v13;
INSERT INTO invalid_attempt_counters SELECT * FROM invalid_attempt_counters_seat_v13;
INSERT INTO agent_run_results SELECT * FROM agent_run_results_seat_v13;
INSERT INTO game_replay_agents SELECT * FROM game_replay_agents_seat_v13;
INSERT INTO agent_run_budget_telemetry SELECT * FROM agent_run_budget_telemetry_seat_v13;
INSERT INTO agent_run_telemetry SELECT * FROM agent_run_telemetry_seat_v13;

DROP TABLE agent_run_budget_telemetry_seat_v13;
DROP TABLE agent_run_telemetry_seat_v13;
DROP TABLE game_replay_agents_seat_v13;
DROP TABLE agent_run_results_seat_v13;
DROP TABLE capabilities_seat_v13;
DROP TABLE audit_records_seat_v13;
DROP TABLE agent_runs_seat_v13;
DROP TABLE private_events_seat_v13;
DROP TABLE turn_deadlines_seat_v13;
DROP TABLE invalid_attempt_counters_seat_v13;
DROP TABLE seats_seat_v13;

CREATE INDEX private_events_seat_idx
    ON private_events (game_id, seat_number, sequence);
CREATE INDEX agent_runs_status_idx ON agent_runs (status, created_at_ms, run_id);
CREATE INDEX agent_runs_manifest_idx ON agent_runs (manifest_sha256, created_at_ms);
CREATE UNIQUE INDEX agent_runs_result_identity_idx
    ON agent_runs (run_id, manifest_sha256);
CREATE UNIQUE INDEX agent_runs_replay_identity_idx
    ON agent_runs (run_id, game_id, seat_number, manifest_sha256);
CREATE INDEX capabilities_active_idx
    ON capabilities (game_id, authority_kind, seat_number, expires_at_ms, revoked_at_ms);
CREATE INDEX capabilities_agent_run_idx
    ON capabilities (agent_run_id, issued_at_ms, capability_id)
    WHERE agent_run_id IS NOT NULL;
CREATE INDEX audit_records_game_time_idx
    ON audit_records (game_id, occurred_at_ms, audit_id);
CREATE INDEX game_replay_agents_manifest_idx
    ON game_replay_agents (manifest_sha256, game_id, version, seat_number);
CREATE INDEX agent_run_telemetry_correlation_idx ON agent_run_telemetry (
    tournament_id, match_id, game_id, run_id
);
CREATE INDEX agent_run_telemetry_expiry_idx ON agent_run_telemetry (
    expires_at_ms, run_id
) WHERE expires_at_ms IS NOT NULL;

UPDATE schema_metadata
SET value = '14', updated_at_ms = 0
WHERE key = 'application_schema_version';

PRAGMA foreign_keys = ON;
