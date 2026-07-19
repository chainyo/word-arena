CREATE TABLE scheduler_limits (
    scope_key TEXT PRIMARY KEY NOT NULL,
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    scope_json BLOB NOT NULL CHECK (length(scope_json) > 0),
    max_concurrency INTEGER NOT NULL CHECK (max_concurrency BETWEEN 1 AND 10000),
    rate_capacity INTEGER CHECK (rate_capacity BETWEEN 1 AND 1000000),
    refill_tokens INTEGER CHECK (refill_tokens BETWEEN 1 AND 1000000),
    refill_interval_ms INTEGER CHECK (refill_interval_ms BETWEEN 1 AND 86400000),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    CHECK ((rate_capacity IS NULL) = (refill_tokens IS NULL)),
    CHECK ((rate_capacity IS NULL) = (refill_interval_ms IS NULL))
) STRICT;

CREATE TABLE scheduler_buckets (
    scope_key TEXT PRIMARY KEY NOT NULL,
    tokens INTEGER NOT NULL CHECK (tokens >= 0),
    refill_remainder INTEGER NOT NULL CHECK (refill_remainder >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0),
    FOREIGN KEY (scope_key) REFERENCES scheduler_limits (scope_key)
        ON DELETE CASCADE
) STRICT;

CREATE TABLE tournament_worker_controls (
    tournament_id TEXT PRIMARY KEY NOT NULL,
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    control TEXT NOT NULL CHECK (control IN ('running', 'paused', 'draining', 'cancelled')),
    sequence INTEGER NOT NULL CHECK (sequence >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
) STRICT;

CREATE TABLE execution_reservations (
    reservation_id TEXT PRIMARY KEY NOT NULL,
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    job_id TEXT NOT NULL,
    tournament_id TEXT NOT NULL,
    match_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    harness_id TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    immutable_inputs_sha256 TEXT NOT NULL CHECK (
        length(immutable_inputs_sha256) = 64
        AND immutable_inputs_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    owner TEXT NOT NULL,
    request_json BLOB NOT NULL CHECK (length(request_json) > 0),
    status TEXT NOT NULL CHECK (
        status IN ('active', 'cancel_requested', 'released', 'expired', 'completed')
    ),
    acquired_at_ms INTEGER NOT NULL CHECK (acquired_at_ms >= 0),
    expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms > acquired_at_ms),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= acquired_at_ms),
    finished_at_ms INTEGER CHECK (finished_at_ms IS NULL OR finished_at_ms >= acquired_at_ms),
    CHECK ((status IN ('active', 'cancel_requested')) = (finished_at_ms IS NULL))
) STRICT;

CREATE UNIQUE INDEX execution_reservations_live_match_idx
    ON execution_reservations (match_id)
    WHERE status IN ('active', 'cancel_requested');
CREATE INDEX execution_reservations_expiry_idx
    ON execution_reservations (status, expires_at_ms, match_id);

CREATE TABLE execution_reservation_scopes (
    reservation_id TEXT NOT NULL,
    scope_key TEXT NOT NULL,
    PRIMARY KEY (reservation_id, scope_key),
    FOREIGN KEY (reservation_id) REFERENCES execution_reservations (reservation_id)
        ON DELETE CASCADE,
    FOREIGN KEY (scope_key) REFERENCES scheduler_limits (scope_key)
        ON DELETE RESTRICT
) STRICT;

CREATE INDEX execution_reservation_scopes_limit_idx
    ON execution_reservation_scopes (scope_key, reservation_id);

CREATE TABLE terminal_match_results (
    match_id TEXT PRIMARY KEY NOT NULL,
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    immutable_inputs_sha256 TEXT NOT NULL CHECK (length(immutable_inputs_sha256) = 64),
    result_sha256 TEXT NOT NULL CHECK (length(result_sha256) = 64),
    charge_key TEXT NOT NULL UNIQUE,
    telemetry_key TEXT NOT NULL UNIQUE,
    rating_key TEXT NOT NULL UNIQUE,
    result_json BLOB NOT NULL CHECK (length(result_json) > 0),
    reservation_id TEXT NOT NULL UNIQUE,
    committed_at_ms INTEGER NOT NULL CHECK (committed_at_ms >= 0),
    FOREIGN KEY (reservation_id) REFERENCES execution_reservations (reservation_id)
        ON DELETE RESTRICT
) STRICT;

UPDATE schema_metadata
SET value = '10', updated_at_ms = 0
WHERE key = 'application_schema_version';
