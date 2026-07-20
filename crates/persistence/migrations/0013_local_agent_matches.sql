CREATE TABLE local_agent_matches (
    game_id TEXT PRIMARY KEY NOT NULL,
    status_schema_version INTEGER NOT NULL CHECK (status_schema_version > 0),
    status_json BLOB NOT NULL CHECK (
        length(status_json) > 0 AND length(status_json) <= 262144
    ),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= created_at_ms),
    FOREIGN KEY (game_id) REFERENCES games (game_id)
        ON UPDATE RESTRICT ON DELETE CASCADE
) STRICT;

CREATE INDEX local_agent_matches_recent_idx ON local_agent_matches (
    updated_at_ms DESC, game_id
);

UPDATE schema_metadata
SET value = '13', updated_at_ms = 0
WHERE key = 'application_schema_version';
