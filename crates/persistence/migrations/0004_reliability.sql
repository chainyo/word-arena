CREATE TABLE turn_deadlines (
    game_id TEXT NOT NULL,
    turn_number INTEGER NOT NULL CHECK (turn_number >= 0),
    seat_number INTEGER NOT NULL CHECK (seat_number IN (1, 2)),
    deadline_at_ms INTEGER NOT NULL CHECK (deadline_at_ms >= 0),
    policy_version INTEGER NOT NULL CHECK (policy_version > 0),
    PRIMARY KEY (game_id, turn_number),
    FOREIGN KEY (game_id) REFERENCES games (game_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE creation_idempotency_records (
    key_digest BLOB PRIMARY KEY NOT NULL CHECK (length(key_digest) = 32),
    digest_version INTEGER NOT NULL CHECK (digest_version > 0),
    payload_sha256 TEXT NOT NULL CHECK (
        length(payload_sha256) = 64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    game_id TEXT NOT NULL UNIQUE,
    outcome_json BLOB NOT NULL CHECK (length(outcome_json) > 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    FOREIGN KEY (game_id) REFERENCES games (game_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE invalid_attempt_counters (
    game_id TEXT NOT NULL,
    turn_number INTEGER NOT NULL CHECK (turn_number >= 0),
    seat_number INTEGER NOT NULL CHECK (seat_number IN (1, 2)),
    policy_version INTEGER NOT NULL CHECK (policy_version > 0),
    attempt_count INTEGER NOT NULL CHECK (attempt_count > 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0),
    PRIMARY KEY (game_id, turn_number, seat_number),
    FOREIGN KEY (game_id) REFERENCES games (game_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE game_replays (
    game_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version >= 0),
    replay_schema_version INTEGER NOT NULL CHECK (replay_schema_version > 0),
    payload_json BLOB NOT NULL CHECK (length(payload_json) > 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    PRIMARY KEY (game_id, version),
    FOREIGN KEY (game_id) REFERENCES games (game_id) ON DELETE CASCADE
) STRICT;

UPDATE schema_metadata
SET value = '4', updated_at_ms = 0
WHERE key = 'application_schema_version';
