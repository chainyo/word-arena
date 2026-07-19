CREATE TABLE tournaments (
    tournament_id TEXT PRIMARY KEY NOT NULL,
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    format_kind TEXT NOT NULL CHECK (
        format_kind IN ('round_robin', 'paired_seat_swap', 'swiss', 'series')
    ),
    status TEXT NOT NULL CHECK (
        status IN ('draft', 'scheduled', 'running', 'paused', 'finished', 'cancelled')
    ),
    config_json BLOB NOT NULL CHECK (length(config_json) > 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms)
) STRICT;

CREATE INDEX tournaments_status_idx
    ON tournaments (status, updated_at_ms, tournament_id);

CREATE TABLE tournament_entries (
    tournament_id TEXT NOT NULL,
    entrant_id TEXT NOT NULL,
    seed_number INTEGER NOT NULL CHECK (seed_number > 0),
    manifest_sha256 TEXT,
    PRIMARY KEY (tournament_id, entrant_id),
    UNIQUE (tournament_id, seed_number),
    FOREIGN KEY (tournament_id) REFERENCES tournaments (tournament_id) ON DELETE CASCADE,
    FOREIGN KEY (manifest_sha256) REFERENCES agent_manifests (manifest_sha256)
        ON DELETE RESTRICT
) STRICT;

CREATE TABLE matches (
    match_id TEXT PRIMARY KEY NOT NULL,
    tournament_id TEXT,
    sequence INTEGER CHECK (sequence IS NULL OR sequence >= 0),
    game_id TEXT UNIQUE,
    language TEXT NOT NULL CHECK (length(language) BETWEEN 2 AND 35),
    ruleset_id TEXT NOT NULL,
    ruleset_sha256 TEXT NOT NULL,
    lexicon_pack_id TEXT NOT NULL,
    lexicon_pack_version TEXT NOT NULL,
    lexicon_content_sha256 TEXT NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN ('pending', 'running', 'finished', 'failed', 'cancelled')
    ),
    scheduled_at_ms INTEGER CHECK (scheduled_at_ms IS NULL OR scheduled_at_ms >= 0),
    started_at_ms INTEGER CHECK (started_at_ms IS NULL OR started_at_ms >= 0),
    finished_at_ms INTEGER CHECK (finished_at_ms IS NULL OR finished_at_ms >= 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    UNIQUE (tournament_id, sequence),
    FOREIGN KEY (tournament_id) REFERENCES tournaments (tournament_id) ON DELETE CASCADE,
    FOREIGN KEY (game_id) REFERENCES games (game_id) ON DELETE RESTRICT,
    FOREIGN KEY (ruleset_id, ruleset_sha256)
        REFERENCES rulesets (ruleset_id, content_sha256) ON DELETE RESTRICT,
    FOREIGN KEY (lexicon_pack_id, lexicon_pack_version, lexicon_content_sha256)
        REFERENCES lexicon_packs (pack_id, pack_version, content_sha256) ON DELETE RESTRICT,
    CHECK (finished_at_ms IS NULL OR started_at_ms IS NOT NULL),
    CHECK (started_at_ms IS NULL OR started_at_ms >= created_at_ms),
    CHECK (finished_at_ms IS NULL OR finished_at_ms >= started_at_ms)
) STRICT;

CREATE INDEX matches_tournament_status_idx
    ON matches (tournament_id, status, sequence, match_id);

CREATE TABLE agent_manifests (
    manifest_sha256 TEXT PRIMARY KEY NOT NULL CHECK (
        length(manifest_sha256) = 64 AND manifest_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    manifest_json BLOB NOT NULL CHECK (length(manifest_json) > 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0)
) STRICT;

CREATE TABLE agent_runs (
    run_id TEXT PRIMARY KEY NOT NULL,
    match_id TEXT,
    game_id TEXT NOT NULL,
    seat_number INTEGER NOT NULL CHECK (seat_number IN (1, 2)),
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

CREATE INDEX agent_runs_status_idx ON agent_runs (status, created_at_ms, run_id);
CREATE INDEX agent_runs_manifest_idx ON agent_runs (manifest_sha256, created_at_ms);

CREATE TABLE capabilities (
    capability_id TEXT PRIMARY KEY NOT NULL,
    game_id TEXT NOT NULL,
    seat_number INTEGER CHECK (seat_number IN (1, 2)),
    authority_kind TEXT NOT NULL CHECK (
        authority_kind IN ('public', 'seat', 'human_spectator', 'administrator')
    ),
    scopes TEXT NOT NULL CHECK (length(scopes) > 0),
    token_digest BLOB NOT NULL CHECK (length(token_digest) = 32),
    digest_version INTEGER NOT NULL CHECK (digest_version > 0),
    issued_at_ms INTEGER NOT NULL CHECK (issued_at_ms >= 0),
    expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms > issued_at_ms),
    revoked_at_ms INTEGER CHECK (revoked_at_ms IS NULL OR revoked_at_ms >= issued_at_ms),
    UNIQUE (token_digest),
    FOREIGN KEY (game_id) REFERENCES games (game_id) ON DELETE CASCADE,
    FOREIGN KEY (game_id, seat_number) REFERENCES seats (game_id, seat_number)
        ON DELETE CASCADE,
    CHECK (
        (authority_kind = 'seat' AND seat_number IS NOT NULL)
        OR (authority_kind <> 'seat' AND seat_number IS NULL)
    )
) STRICT;

CREATE INDEX capabilities_active_idx
    ON capabilities (game_id, authority_kind, seat_number, expires_at_ms, revoked_at_ms);

CREATE TABLE idempotency_records (
    game_id TEXT NOT NULL,
    key_digest BLOB NOT NULL CHECK (length(key_digest) = 32),
    digest_version INTEGER NOT NULL CHECK (digest_version > 0),
    command_kind TEXT NOT NULL CHECK (length(command_kind) > 0),
    payload_sha256 TEXT NOT NULL CHECK (
        length(payload_sha256) = 64 AND payload_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    outcome_kind TEXT NOT NULL CHECK (outcome_kind IN ('accepted', 'rejected')),
    outcome_json BLOB NOT NULL CHECK (length(outcome_json) > 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    PRIMARY KEY (game_id, key_digest),
    FOREIGN KEY (game_id) REFERENCES games (game_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE audit_records (
    audit_id INTEGER PRIMARY KEY AUTOINCREMENT,
    game_id TEXT,
    actor_kind TEXT NOT NULL CHECK (
        actor_kind IN ('public', 'seat', 'human_spectator', 'administrator', 'system')
    ),
    seat_number INTEGER CHECK (seat_number IN (1, 2)),
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

CREATE INDEX audit_records_game_time_idx
    ON audit_records (game_id, occurred_at_ms, audit_id);

UPDATE schema_metadata
SET value = '2', updated_at_ms = 0
WHERE key = 'application_schema_version';
