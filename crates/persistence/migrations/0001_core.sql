PRAGMA foreign_keys = ON;

CREATE TABLE schema_metadata (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
) STRICT;

INSERT INTO schema_metadata (key, value, updated_at_ms)
VALUES ('application_schema_version', '1', 0);

CREATE TABLE rulesets (
    ruleset_id TEXT NOT NULL,
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    content_sha256 TEXT NOT NULL CHECK (
        length(content_sha256) = 64 AND content_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    definition_json BLOB NOT NULL CHECK (length(definition_json) > 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    PRIMARY KEY (ruleset_id, content_sha256)
) STRICT;

CREATE TABLE lexicon_packs (
    pack_id TEXT NOT NULL,
    pack_version TEXT NOT NULL,
    content_sha256 TEXT NOT NULL CHECK (
        length(content_sha256) = 64 AND content_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    format_version INTEGER NOT NULL CHECK (format_version > 0),
    normalization_version INTEGER NOT NULL CHECK (normalization_version > 0),
    locale TEXT NOT NULL CHECK (length(locale) BETWEEN 2 AND 35),
    identity_json BLOB NOT NULL CHECK (length(identity_json) > 0),
    installed_at_ms INTEGER NOT NULL CHECK (installed_at_ms >= 0),
    PRIMARY KEY (pack_id, pack_version, content_sha256)
) STRICT;

CREATE TABLE games (
    game_id TEXT PRIMARY KEY NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'finished')),
    version INTEGER NOT NULL CHECK (version >= 0),
    ruleset_id TEXT NOT NULL,
    ruleset_sha256 TEXT NOT NULL,
    lexicon_pack_id TEXT NOT NULL,
    lexicon_pack_version TEXT NOT NULL,
    lexicon_content_sha256 TEXT NOT NULL,
    rng_algorithm TEXT NOT NULL,
    seed_commitment_sha256 TEXT NOT NULL CHECK (
        length(seed_commitment_sha256) = 64
        AND seed_commitment_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    finished_at_ms INTEGER CHECK (
        finished_at_ms IS NULL OR finished_at_ms >= created_at_ms
    ),
    FOREIGN KEY (ruleset_id, ruleset_sha256)
        REFERENCES rulesets (ruleset_id, content_sha256) ON UPDATE RESTRICT ON DELETE RESTRICT,
    FOREIGN KEY (lexicon_pack_id, lexicon_pack_version, lexicon_content_sha256)
        REFERENCES lexicon_packs (pack_id, pack_version, content_sha256)
        ON UPDATE RESTRICT ON DELETE RESTRICT
) STRICT;

CREATE INDEX games_status_updated_idx ON games (status, updated_at_ms, game_id);
CREATE INDEX games_ruleset_idx ON games (ruleset_id, ruleset_sha256);
CREATE INDEX games_lexicon_idx ON games (
    lexicon_pack_id, lexicon_pack_version, lexicon_content_sha256
);

CREATE TABLE seats (
    game_id TEXT NOT NULL,
    seat_number INTEGER NOT NULL CHECK (seat_number IN (1, 2)),
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

CREATE TABLE public_events (
    game_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence >= 0),
    event_schema_version INTEGER NOT NULL CHECK (event_schema_version > 0),
    payload_json BLOB NOT NULL CHECK (length(payload_json) > 0),
    committed_at_ms INTEGER NOT NULL CHECK (committed_at_ms >= 0),
    PRIMARY KEY (game_id, sequence),
    FOREIGN KEY (game_id) REFERENCES games (game_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX public_events_game_time_idx
    ON public_events (game_id, committed_at_ms, sequence);

CREATE TABLE private_events (
    game_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    seat_number INTEGER NOT NULL CHECK (seat_number IN (1, 2)),
    event_schema_version INTEGER NOT NULL CHECK (event_schema_version > 0),
    payload_json BLOB NOT NULL CHECK (length(payload_json) > 0),
    committed_at_ms INTEGER NOT NULL CHECK (committed_at_ms >= 0),
    PRIMARY KEY (game_id, sequence, seat_number),
    FOREIGN KEY (game_id, seat_number) REFERENCES seats (game_id, seat_number)
        ON DELETE CASCADE,
    FOREIGN KEY (game_id, sequence) REFERENCES public_events (game_id, sequence)
        ON DELETE CASCADE
) STRICT;

CREATE INDEX private_events_seat_idx
    ON private_events (game_id, seat_number, sequence);

CREATE TABLE game_snapshots (
    game_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version >= 0),
    snapshot_schema_version INTEGER NOT NULL CHECK (snapshot_schema_version > 0),
    payload_json BLOB NOT NULL CHECK (length(payload_json) > 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    PRIMARY KEY (game_id, version),
    FOREIGN KEY (game_id) REFERENCES games (game_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX game_snapshots_latest_idx ON game_snapshots (game_id, version DESC);
