CREATE TABLE tournament_schedules (
    tournament_id TEXT PRIMARY KEY NOT NULL,
    format_schema_version INTEGER NOT NULL CHECK (format_schema_version > 0),
    schedule_schema_version INTEGER NOT NULL CHECK (schedule_schema_version > 0),
    lifecycle_schema_version INTEGER NOT NULL CHECK (lifecycle_schema_version > 0),
    format_identity_sha256 TEXT NOT NULL CHECK (
        length(format_identity_sha256) = 64
        AND format_identity_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    spec_json BLOB NOT NULL CHECK (length(spec_json) > 0),
    schedule_json BLOB NOT NULL CHECK (length(schedule_json) > 0),
    swiss_progress_json BLOB,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    FOREIGN KEY (tournament_id) REFERENCES tournaments (tournament_id)
        ON UPDATE RESTRICT ON DELETE CASCADE
) STRICT;

CREATE TABLE tournament_series (
    series_id TEXT PRIMARY KEY NOT NULL,
    tournament_id TEXT NOT NULL,
    round_number INTEGER NOT NULL CHECK (round_number > 0),
    table_number INTEGER NOT NULL CHECK (table_number > 0),
    entrant_a TEXT NOT NULL,
    entrant_b TEXT NOT NULL,
    match_count INTEGER NOT NULL CHECK (match_count > 0),
    status TEXT NOT NULL CHECK (
        status IN ('pending', 'running', 'finished', 'failed', 'cancelled')
    ),
    UNIQUE (series_id, tournament_id),
    UNIQUE (tournament_id, round_number, table_number),
    CHECK (entrant_a <> entrant_b),
    FOREIGN KEY (tournament_id) REFERENCES tournaments (tournament_id)
        ON UPDATE RESTRICT ON DELETE CASCADE,
    FOREIGN KEY (tournament_id, entrant_a)
        REFERENCES tournament_entries (tournament_id, entrant_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (tournament_id, entrant_b)
        REFERENCES tournament_entries (tournament_id, entrant_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT;

CREATE TABLE tournament_match_schedule (
    match_id TEXT PRIMARY KEY NOT NULL,
    tournament_id TEXT NOT NULL,
    series_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence >= 0),
    round_number INTEGER NOT NULL CHECK (round_number > 0),
    table_number INTEGER NOT NULL CHECK (table_number > 0),
    series_game_number INTEGER NOT NULL CHECK (series_game_number > 0),
    game_seed_commitment_sha256 TEXT NOT NULL CHECK (
        length(game_seed_commitment_sha256) = 64
        AND game_seed_commitment_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    format_identity_sha256 TEXT NOT NULL CHECK (
        length(format_identity_sha256) = 64
        AND format_identity_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    UNIQUE (match_id, tournament_id),
    UNIQUE (series_id, series_game_number),
    UNIQUE (tournament_id, sequence),
    FOREIGN KEY (match_id) REFERENCES matches (match_id)
        ON UPDATE RESTRICT ON DELETE CASCADE,
    FOREIGN KEY (series_id, tournament_id)
        REFERENCES tournament_series (series_id, tournament_id)
        ON UPDATE RESTRICT ON DELETE CASCADE
) STRICT;

CREATE TABLE tournament_match_seats (
    match_id TEXT NOT NULL,
    tournament_id TEXT NOT NULL,
    seat_number INTEGER NOT NULL CHECK (seat_number IN (1, 2)),
    entrant_id TEXT NOT NULL,
    PRIMARY KEY (match_id, seat_number),
    UNIQUE (match_id, entrant_id),
    FOREIGN KEY (match_id, tournament_id)
        REFERENCES tournament_match_schedule (match_id, tournament_id)
        ON UPDATE RESTRICT ON DELETE CASCADE,
    FOREIGN KEY (tournament_id, entrant_id)
        REFERENCES tournament_entries (tournament_id, entrant_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT;

CREATE TABLE tournament_byes (
    tournament_id TEXT NOT NULL,
    round_number INTEGER NOT NULL CHECK (round_number > 0),
    entrant_id TEXT NOT NULL,
    PRIMARY KEY (tournament_id, round_number),
    FOREIGN KEY (tournament_id, entrant_id)
        REFERENCES tournament_entries (tournament_id, entrant_id)
        ON UPDATE RESTRICT ON DELETE CASCADE
) STRICT;

CREATE TABLE tournament_lifecycle_events (
    tournament_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence >= 0),
    lifecycle_schema_version INTEGER NOT NULL CHECK (lifecycle_schema_version > 0),
    state TEXT NOT NULL CHECK (
        state IN ('draft', 'scheduled', 'running', 'paused', 'finished', 'cancelled')
    ),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0),
    PRIMARY KEY (tournament_id, sequence),
    FOREIGN KEY (tournament_id) REFERENCES tournaments (tournament_id)
        ON UPDATE RESTRICT ON DELETE CASCADE
) STRICT;

CREATE INDEX tournament_series_round_idx
    ON tournament_series (tournament_id, round_number, table_number);

CREATE INDEX tournament_match_schedule_round_idx
    ON tournament_match_schedule (
        tournament_id, round_number, series_game_number, table_number
    );

UPDATE schema_metadata
SET value = '8', updated_at_ms = 0
WHERE key = 'application_schema_version';
