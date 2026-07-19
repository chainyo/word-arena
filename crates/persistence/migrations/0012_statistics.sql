CREATE TABLE statistics_sources (
    source_id TEXT PRIMARY KEY NOT NULL,
    statistics_schema_version INTEGER NOT NULL CHECK (statistics_schema_version > 0),
    tournament_id TEXT,
    match_id TEXT NOT NULL,
    game_id TEXT NOT NULL,
    finished_at_ms INTEGER NOT NULL CHECK (finished_at_ms >= 0),
    source_json BLOB NOT NULL CHECK (length(source_json) > 0),
    observations_json BLOB NOT NULL CHECK (length(observations_json) > 0),
    recorded_at_ms INTEGER NOT NULL CHECK (recorded_at_ms >= 0)
) STRICT;

CREATE TABLE statistics_observations (
    observation_id TEXT PRIMARY KEY NOT NULL,
    source_id TEXT NOT NULL,
    statistics_schema_version INTEGER NOT NULL CHECK (statistics_schema_version > 0),
    language TEXT NOT NULL,
    ruleset_id TEXT NOT NULL,
    ruleset_sha256 TEXT NOT NULL CHECK (
        length(ruleset_sha256) = 64 AND ruleset_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    pack_id TEXT NOT NULL,
    pack_version TEXT NOT NULL,
    pack_sha256 TEXT NOT NULL CHECK (
        length(pack_sha256) = 64 AND pack_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    agent_manifest_sha256 TEXT CHECK (
        agent_manifest_sha256 IS NULL OR (
            length(agent_manifest_sha256) = 64
            AND agent_manifest_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    tournament_id TEXT,
    match_id TEXT NOT NULL,
    game_id TEXT NOT NULL,
    entrant_id TEXT NOT NULL,
    seat_number INTEGER NOT NULL CHECK (seat_number IN (1, 2)),
    finished_at_ms INTEGER NOT NULL CHECK (finished_at_ms >= 0),
    observation_json BLOB NOT NULL CHECK (length(observation_json) > 0),
    UNIQUE (source_id, seat_number),
    FOREIGN KEY (source_id) REFERENCES statistics_sources (source_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT;

CREATE INDEX statistics_observations_scope_idx ON statistics_observations (
    language, ruleset_id, ruleset_sha256, pack_id, pack_version, pack_sha256,
    agent_manifest_sha256, tournament_id, entrant_id, seat_number, finished_at_ms,
    observation_id
);

UPDATE schema_metadata
SET value = '12', updated_at_ms = 0
WHERE key = 'application_schema_version';
