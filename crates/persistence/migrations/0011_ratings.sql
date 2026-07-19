CREATE TABLE rating_periods (
    period_id TEXT PRIMARY KEY NOT NULL,
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    pool_key TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence >= 0),
    pool_json BLOB NOT NULL CHECK (length(pool_json) > 0),
    period_json BLOB NOT NULL CHECK (length(period_json) > 0),
    derived_json BLOB NOT NULL CHECK (length(derived_json) > 0),
    committed_at_ms INTEGER NOT NULL CHECK (committed_at_ms >= 0),
    UNIQUE (pool_key, sequence)
) STRICT;

CREATE TABLE rating_match_inputs (
    pool_key TEXT NOT NULL,
    match_id TEXT NOT NULL,
    period_id TEXT NOT NULL,
    series_id TEXT NOT NULL,
    series_game_number INTEGER NOT NULL CHECK (series_game_number > 0),
    entrant_one TEXT NOT NULL,
    entrant_two TEXT NOT NULL,
    score_one_millionths INTEGER NOT NULL CHECK (score_one_millionths BETWEEN 0 AND 1000000),
    PRIMARY KEY (pool_key, match_id),
    FOREIGN KEY (period_id) REFERENCES rating_periods (period_id) ON DELETE RESTRICT,
    CHECK (entrant_one <> entrant_two)
) STRICT;

CREATE TABLE rating_period_inputs (
    period_id TEXT NOT NULL,
    entrant_id TEXT NOT NULL,
    previous_rating_milli INTEGER NOT NULL,
    previous_deviation_milli INTEGER NOT NULL CHECK (previous_deviation_milli > 0),
    previous_volatility_nano INTEGER NOT NULL CHECK (previous_volatility_nano > 0),
    opponents_json BLOB NOT NULL CHECK (length(opponents_json) > 0),
    PRIMARY KEY (period_id, entrant_id),
    FOREIGN KEY (period_id) REFERENCES rating_periods (period_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE rating_updates (
    period_id TEXT NOT NULL,
    entrant_id TEXT NOT NULL,
    pool_key TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence >= 0),
    rating_milli INTEGER NOT NULL,
    deviation_milli INTEGER NOT NULL CHECK (deviation_milli > 0),
    volatility_nano INTEGER NOT NULL CHECK (volatility_nano > 0),
    PRIMARY KEY (period_id, entrant_id),
    UNIQUE (pool_key, sequence, entrant_id),
    FOREIGN KEY (period_id, entrant_id)
        REFERENCES rating_period_inputs (period_id, entrant_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE current_ratings (
    pool_key TEXT NOT NULL,
    entrant_id TEXT NOT NULL,
    period_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence >= 0),
    rating_milli INTEGER NOT NULL,
    deviation_milli INTEGER NOT NULL CHECK (deviation_milli > 0),
    volatility_nano INTEGER NOT NULL CHECK (volatility_nano > 0),
    PRIMARY KEY (pool_key, entrant_id),
    FOREIGN KEY (period_id, entrant_id)
        REFERENCES rating_updates (period_id, entrant_id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX rating_periods_pool_idx ON rating_periods (pool_key, sequence);

UPDATE schema_metadata
SET value = '11', updated_at_ms = 0
WHERE key = 'application_schema_version';
