use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use word_arena_application::{
    BoxFuture, GameId, GameRepository, RepositoryError, StoredGame, UnixMillis,
};
use word_arena_engine::{
    GameEvent, GameEventKind, GamePhase, GameSnapshot, PrivateGameEvent, Ruleset,
    SNAPSHOT_SCHEMA_VERSION,
};
use word_arena_lexicon::PackIdentity;

const EVENT_SCHEMA_VERSION: i64 = 1;

/// SQLx-backed optimistic game repository.
#[derive(Clone, Debug)]
pub struct SqliteGameRepository {
    pool: SqlitePool,
}

impl SqliteGameRepository {
    /// Wraps an already migrated pool.
    #[must_use]
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Underlying pool for other focused `SQLite` adapters.
    #[must_use]
    pub const fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    async fn insert_game(&self, record: StoredGame) -> Result<(), RepositoryError> {
        let ruleset = initial_ruleset(&record)?;
        let mut transaction = self.pool.begin().await.map_err(map_transient)?;
        register_ruleset(&mut transaction, ruleset, record.created_at).await?;
        register_lexicon(
            &mut transaction,
            &record.snapshot.state.lexicon,
            record.created_at,
        )
        .await?;

        let snapshot =
            serde_json::to_vec(&record.snapshot).map_err(|_| RepositoryError::Corrupt)?;
        let event =
            serde_json::to_vec(&record.snapshot.events[0]).map_err(|_| RepositoryError::Corrupt)?;
        let state = &record.snapshot.state;
        let inserted = sqlx::query(
            "INSERT INTO games (
                game_id, status, version, ruleset_id, ruleset_sha256,
                lexicon_pack_id, lexicon_pack_version, lexicon_content_sha256,
                rng_algorithm, seed_commitment_sha256, created_at_ms, updated_at_ms,
                finished_at_ms
             ) VALUES (?, 'active', 0, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
        )
        .bind(record.game_id.as_str())
        .bind(state.ruleset_id.as_str())
        .bind(&state.ruleset.content_sha256)
        .bind(&state.lexicon.pack_id)
        .bind(&state.lexicon.pack_version)
        .bind(&state.lexicon.content_sha256)
        .bind(state.rng_algorithm.as_str())
        .bind(&state.seed_commitment.sha256)
        .bind(record.created_at.0)
        .bind(record.updated_at.0)
        .execute(&mut *transaction)
        .await
        .map_err(map_game_insert)?;
        if inserted.rows_affected() != 1 {
            return Err(RepositoryError::Unavailable);
        }
        for seat in [1_i64, 2] {
            sqlx::query(
                "INSERT INTO seats (
                    game_id, seat_number, participant_kind, created_at_ms
                 ) VALUES (?, ?, 'unassigned', ?)",
            )
            .bind(record.game_id.as_str())
            .bind(seat)
            .bind(record.created_at.0)
            .execute(&mut *transaction)
            .await
            .map_err(map_write)?;
        }
        insert_public_event(
            &mut transaction,
            record.game_id.as_str(),
            &record.snapshot.events[0],
            &event,
            record.created_at,
        )
        .await?;
        insert_snapshot(
            &mut transaction,
            record.game_id.as_str(),
            &record.snapshot,
            &snapshot,
            record.created_at,
        )
        .await?;
        transaction.commit().await.map_err(map_transient)
    }

    async fn load_game(&self, game_id: &GameId) -> Result<StoredGame, RepositoryError> {
        let row = sqlx::query(
            "SELECT
                g.status, g.version, g.ruleset_id, g.ruleset_sha256,
                g.lexicon_pack_id, g.lexicon_pack_version, g.lexicon_content_sha256,
                g.created_at_ms, g.updated_at_ms, s.payload_json,
                r.definition_json, l.identity_json
             FROM games AS g
             JOIN game_snapshots AS s
               ON s.game_id = g.game_id AND s.version = g.version
             JOIN rulesets AS r
               ON r.ruleset_id = g.ruleset_id AND r.content_sha256 = g.ruleset_sha256
             JOIN lexicon_packs AS l
               ON l.pack_id = g.lexicon_pack_id
              AND l.pack_version = g.lexicon_pack_version
              AND l.content_sha256 = g.lexicon_content_sha256
             WHERE g.game_id = ?",
        )
        .bind(game_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_read)?
        .ok_or(RepositoryError::NotFound)?;

        let snapshot_bytes: Vec<u8> = row
            .try_get("payload_json")
            .map_err(|_| RepositoryError::Corrupt)?;
        let snapshot: GameSnapshot =
            serde_json::from_slice(&snapshot_bytes).map_err(|_| RepositoryError::Corrupt)?;
        if snapshot.schema_version != SNAPSHOT_SCHEMA_VERSION {
            return Err(RepositoryError::IncompatibleSchema);
        }
        let version = nonnegative_u64(
            row.try_get("version")
                .map_err(|_| RepositoryError::Corrupt)?,
        )?;
        let created_at = nonnegative_time(
            row.try_get("created_at_ms")
                .map_err(|_| RepositoryError::Corrupt)?,
        )?;
        let updated_at = nonnegative_time(
            row.try_get("updated_at_ms")
                .map_err(|_| RepositoryError::Corrupt)?,
        )?;
        validate_loaded_metadata(&row, game_id, &snapshot, version)?;
        let ruleset_bytes: Vec<u8> = row
            .try_get("definition_json")
            .map_err(|_| RepositoryError::Corrupt)?;
        validate_ruleset_bytes(&ruleset_bytes, &snapshot)?;
        let lexicon_bytes: Vec<u8> = row
            .try_get("identity_json")
            .map_err(|_| RepositoryError::Corrupt)?;
        validate_lexicon_bytes(&lexicon_bytes, &snapshot.state.lexicon)?;

        let events = load_public_events(&self.pool, game_id).await?;
        let private_events = load_private_events(&self.pool, game_id).await?;
        if events != snapshot.events || private_events != snapshot.private_events {
            return Err(RepositoryError::Corrupt);
        }
        Ok(StoredGame {
            game_id: game_id.clone(),
            created_at,
            updated_at,
            snapshot,
        })
    }

    async fn replace_game(
        &self,
        expected_version: u64,
        record: StoredGame,
    ) -> Result<(), RepositoryError> {
        let next_version = expected_version
            .checked_add(1)
            .ok_or(RepositoryError::Corrupt)?;
        if record.snapshot.state.version != next_version
            || record.updated_at < record.created_at
            || record.snapshot.state.game_id != record.game_id.as_str()
        {
            return Err(RepositoryError::Corrupt);
        }
        let next_version_i64 = i64::try_from(next_version).map_err(|_| RepositoryError::Corrupt)?;
        let expected_i64 = i64::try_from(expected_version).map_err(|_| RepositoryError::Corrupt)?;
        let status = phase_name(record.snapshot.state.phase);
        let finished_at =
            (record.snapshot.state.phase == GamePhase::Finished).then_some(record.updated_at.0);
        let mut transaction = self.pool.begin().await.map_err(map_transient)?;
        let updated = sqlx::query(
            "UPDATE games
             SET version = ?, status = ?, updated_at_ms = ?, finished_at_ms = ?
             WHERE game_id = ? AND version = ?",
        )
        .bind(next_version_i64)
        .bind(status)
        .bind(record.updated_at.0)
        .bind(finished_at)
        .bind(record.game_id.as_str())
        .bind(expected_i64)
        .execute(&mut *transaction)
        .await
        .map_err(map_write)?;
        if updated.rows_affected() == 0 {
            let exists =
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM games WHERE game_id = ?")
                    .bind(record.game_id.as_str())
                    .fetch_one(&mut *transaction)
                    .await
                    .map_err(map_read)?;
            return Err(if exists == 0 {
                RepositoryError::NotFound
            } else {
                RepositoryError::Conflict
            });
        }

        let previous_bytes = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT payload_json FROM game_snapshots WHERE game_id = ? AND version = ?",
        )
        .bind(record.game_id.as_str())
        .bind(expected_i64)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_read)?
        .ok_or(RepositoryError::Corrupt)?;
        let previous: GameSnapshot =
            serde_json::from_slice(&previous_bytes).map_err(|_| RepositoryError::Corrupt)?;
        let (event, private_events) = validate_successor(&previous, &record.snapshot)?;
        let event_bytes = serde_json::to_vec(event).map_err(|_| RepositoryError::Corrupt)?;
        insert_public_event(
            &mut transaction,
            record.game_id.as_str(),
            event,
            &event_bytes,
            record.updated_at,
        )
        .await?;
        for private_event in private_events {
            let bytes = serde_json::to_vec(private_event).map_err(|_| RepositoryError::Corrupt)?;
            insert_private_event(
                &mut transaction,
                record.game_id.as_str(),
                private_event,
                &bytes,
                record.updated_at,
            )
            .await?;
        }
        let snapshot_bytes =
            serde_json::to_vec(&record.snapshot).map_err(|_| RepositoryError::Corrupt)?;
        insert_snapshot(
            &mut transaction,
            record.game_id.as_str(),
            &record.snapshot,
            &snapshot_bytes,
            record.updated_at,
        )
        .await?;
        transaction.commit().await.map_err(map_transient)
    }
}

impl GameRepository for SqliteGameRepository {
    fn insert(&self, game: StoredGame) -> BoxFuture<'_, Result<(), RepositoryError>> {
        Box::pin(self.insert_game(game))
    }

    fn load(&self, game_id: &GameId) -> BoxFuture<'_, Result<StoredGame, RepositoryError>> {
        let game_id = game_id.clone();
        Box::pin(async move { self.load_game(&game_id).await })
    }

    fn replace(
        &self,
        expected_version: u64,
        game: StoredGame,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        Box::pin(self.replace_game(expected_version, game))
    }
}

fn initial_ruleset(record: &StoredGame) -> Result<&Ruleset, RepositoryError> {
    if record.snapshot.schema_version != SNAPSHOT_SCHEMA_VERSION
        || record.snapshot.state.version != 0
        || record.snapshot.state.phase != GamePhase::Active
        || record.snapshot.state.game_id != record.game_id.as_str()
        || record.created_at != record.updated_at
        || record.snapshot.events.len() != 1
        || !record.snapshot.private_events.is_empty()
    {
        return Err(RepositoryError::Corrupt);
    }
    let event = &record.snapshot.events[0];
    let GameEventKind::Created { ruleset, .. } = &event.kind else {
        return Err(RepositoryError::Corrupt);
    };
    if event.sequence != 0
        || ruleset.identity() != record.snapshot.ruleset
        || record.snapshot.state.ruleset != record.snapshot.ruleset
        || ruleset.lexicon != record.snapshot.state.lexicon
    {
        return Err(RepositoryError::Corrupt);
    }
    Ok(ruleset)
}

async fn register_ruleset(
    transaction: &mut Transaction<'_, Sqlite>,
    ruleset: &Ruleset,
    created_at: UnixMillis,
) -> Result<(), RepositoryError> {
    let identity = ruleset.identity();
    let bytes = serde_json::to_vec(ruleset).map_err(|_| RepositoryError::Corrupt)?;
    sqlx::query(
        "INSERT INTO rulesets (
            ruleset_id, schema_version, content_sha256, definition_json, created_at_ms
         ) VALUES (?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
    )
    .bind(ruleset.id.as_str())
    .bind(i64::from(identity.schema_version))
    .bind(&identity.content_sha256)
    .bind(&bytes)
    .bind(created_at.0)
    .execute(&mut **transaction)
    .await
    .map_err(map_write)?;
    let stored = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT definition_json FROM rulesets WHERE ruleset_id = ? AND content_sha256 = ?",
    )
    .bind(ruleset.id.as_str())
    .bind(&identity.content_sha256)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_read)?;
    if stored == bytes {
        Ok(())
    } else {
        Err(RepositoryError::Corrupt)
    }
}

async fn register_lexicon(
    transaction: &mut Transaction<'_, Sqlite>,
    identity: &PackIdentity,
    installed_at: UnixMillis,
) -> Result<(), RepositoryError> {
    let bytes = serde_json::to_vec(identity).map_err(|_| RepositoryError::Corrupt)?;
    sqlx::query(
        "INSERT INTO lexicon_packs (
            pack_id, pack_version, content_sha256, format_version,
            normalization_version, locale, identity_json, installed_at_ms
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
    )
    .bind(&identity.pack_id)
    .bind(&identity.pack_version)
    .bind(&identity.content_sha256)
    .bind(i64::from(identity.format_version))
    .bind(i64::from(identity.normalization.version))
    .bind(&identity.locale)
    .bind(&bytes)
    .bind(installed_at.0)
    .execute(&mut **transaction)
    .await
    .map_err(map_write)?;
    let stored = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT identity_json FROM lexicon_packs
         WHERE pack_id = ? AND pack_version = ? AND content_sha256 = ?",
    )
    .bind(&identity.pack_id)
    .bind(&identity.pack_version)
    .bind(&identity.content_sha256)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_read)?;
    if stored == bytes {
        Ok(())
    } else {
        Err(RepositoryError::IncompatiblePack)
    }
}

async fn insert_public_event(
    transaction: &mut Transaction<'_, Sqlite>,
    game_id: &str,
    event: &GameEvent,
    bytes: &[u8],
    committed_at: UnixMillis,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "INSERT INTO public_events (
            game_id, sequence, event_schema_version, payload_json, committed_at_ms
         ) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(game_id)
    .bind(i64::try_from(event.sequence).map_err(|_| RepositoryError::Corrupt)?)
    .bind(EVENT_SCHEMA_VERSION)
    .bind(bytes)
    .bind(committed_at.0)
    .execute(&mut **transaction)
    .await
    .map_err(map_write)?;
    Ok(())
}

async fn insert_private_event(
    transaction: &mut Transaction<'_, Sqlite>,
    game_id: &str,
    event: &PrivateGameEvent,
    bytes: &[u8],
    committed_at: UnixMillis,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "INSERT INTO private_events (
            game_id, sequence, seat_number, event_schema_version, payload_json, committed_at_ms
         ) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(game_id)
    .bind(i64::try_from(event.sequence).map_err(|_| RepositoryError::Corrupt)?)
    .bind(seat_number(event.seat))
    .bind(EVENT_SCHEMA_VERSION)
    .bind(bytes)
    .bind(committed_at.0)
    .execute(&mut **transaction)
    .await
    .map_err(map_write)?;
    Ok(())
}

async fn insert_snapshot(
    transaction: &mut Transaction<'_, Sqlite>,
    game_id: &str,
    snapshot: &GameSnapshot,
    bytes: &[u8],
    created_at: UnixMillis,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "INSERT INTO game_snapshots (
            game_id, version, snapshot_schema_version, payload_json, created_at_ms
         ) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(game_id)
    .bind(i64::try_from(snapshot.state.version).map_err(|_| RepositoryError::Corrupt)?)
    .bind(i64::from(snapshot.schema_version))
    .bind(bytes)
    .bind(created_at.0)
    .execute(&mut **transaction)
    .await
    .map_err(map_write)?;
    Ok(())
}

fn validate_successor<'a>(
    previous: &GameSnapshot,
    next: &'a GameSnapshot,
) -> Result<(&'a GameEvent, &'a [PrivateGameEvent]), RepositoryError> {
    if previous.schema_version != SNAPSHOT_SCHEMA_VERSION
        || next.schema_version != SNAPSHOT_SCHEMA_VERSION
        || next.ruleset != previous.ruleset
        || next.rng_algorithm != previous.rng_algorithm
        || next.state.lexicon != previous.state.lexicon
        || next.state.version != previous.state.version.saturating_add(1)
        || next.events.len() != previous.events.len() + 1
        || !next.events.starts_with(&previous.events)
        || !next.private_events.starts_with(&previous.private_events)
    {
        return Err(RepositoryError::Corrupt);
    }
    let added_private = &next.private_events[previous.private_events.len()..];
    if added_private.len() > 1
        || added_private
            .iter()
            .any(|event| event.sequence != next.state.version)
    {
        return Err(RepositoryError::Corrupt);
    }
    let event = next.events.last().ok_or(RepositoryError::Corrupt)?;
    if event.sequence != next.state.version {
        return Err(RepositoryError::Corrupt);
    }
    Ok((event, added_private))
}

async fn load_public_events(
    pool: &SqlitePool,
    game_id: &GameId,
) -> Result<Vec<GameEvent>, RepositoryError> {
    let rows = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT payload_json FROM public_events WHERE game_id = ? ORDER BY sequence",
    )
    .bind(game_id.as_str())
    .fetch_all(pool)
    .await
    .map_err(map_read)?;
    rows.into_iter()
        .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| RepositoryError::Corrupt))
        .collect()
}

async fn load_private_events(
    pool: &SqlitePool,
    game_id: &GameId,
) -> Result<Vec<PrivateGameEvent>, RepositoryError> {
    let rows = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT payload_json FROM private_events
         WHERE game_id = ? ORDER BY sequence, seat_number",
    )
    .bind(game_id.as_str())
    .fetch_all(pool)
    .await
    .map_err(map_read)?;
    rows.into_iter()
        .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| RepositoryError::Corrupt))
        .collect()
}

fn validate_loaded_metadata(
    row: &sqlx::sqlite::SqliteRow,
    game_id: &GameId,
    snapshot: &GameSnapshot,
    version: u64,
) -> Result<(), RepositoryError> {
    let status: String = row
        .try_get("status")
        .map_err(|_| RepositoryError::Corrupt)?;
    let expected_status = phase_name(snapshot.state.phase);
    let ruleset_id: String = row
        .try_get("ruleset_id")
        .map_err(|_| RepositoryError::Corrupt)?;
    let ruleset_sha: String = row
        .try_get("ruleset_sha256")
        .map_err(|_| RepositoryError::Corrupt)?;
    let pack_id: String = row
        .try_get("lexicon_pack_id")
        .map_err(|_| RepositoryError::Corrupt)?;
    let pack_version: String = row
        .try_get("lexicon_pack_version")
        .map_err(|_| RepositoryError::Corrupt)?;
    let pack_sha: String = row
        .try_get("lexicon_content_sha256")
        .map_err(|_| RepositoryError::Corrupt)?;
    if snapshot.state.game_id != game_id.as_str()
        || snapshot.state.version != version
        || status != expected_status
        || ruleset_id != snapshot.state.ruleset_id.as_str()
        || ruleset_sha != snapshot.state.ruleset.content_sha256
    {
        return Err(RepositoryError::Corrupt);
    }
    if pack_id != snapshot.state.lexicon.pack_id
        || pack_version != snapshot.state.lexicon.pack_version
        || pack_sha != snapshot.state.lexicon.content_sha256
    {
        return Err(RepositoryError::IncompatiblePack);
    }
    Ok(())
}

fn validate_ruleset_bytes(bytes: &[u8], snapshot: &GameSnapshot) -> Result<(), RepositoryError> {
    let ruleset: Ruleset = serde_json::from_slice(bytes).map_err(|_| RepositoryError::Corrupt)?;
    let Some(GameEvent {
        kind: GameEventKind::Created {
            ruleset: created, ..
        },
        ..
    }) = snapshot.events.first()
    else {
        return Err(RepositoryError::Corrupt);
    };
    if &ruleset == created && ruleset.identity() == snapshot.ruleset {
        Ok(())
    } else {
        Err(RepositoryError::Corrupt)
    }
}

fn validate_lexicon_bytes(bytes: &[u8], expected: &PackIdentity) -> Result<(), RepositoryError> {
    let identity: PackIdentity =
        serde_json::from_slice(bytes).map_err(|_| RepositoryError::Corrupt)?;
    if &identity == expected {
        Ok(())
    } else {
        Err(RepositoryError::IncompatiblePack)
    }
}

const fn phase_name(phase: GamePhase) -> &'static str {
    match phase {
        GamePhase::Active => "active",
        GamePhase::Finished => "finished",
    }
}

const fn seat_number(seat: word_arena_engine::Seat) -> i64 {
    match seat {
        word_arena_engine::Seat::One => 1,
        word_arena_engine::Seat::Two => 2,
    }
}

fn nonnegative_u64(value: i64) -> Result<u64, RepositoryError> {
    u64::try_from(value).map_err(|_| RepositoryError::Corrupt)
}

fn nonnegative_time(value: i64) -> Result<UnixMillis, RepositoryError> {
    if value >= 0 {
        Ok(UnixMillis(value))
    } else {
        Err(RepositoryError::Corrupt)
    }
}

fn map_game_insert(error: sqlx::Error) -> RepositoryError {
    if matches!(&error, sqlx::Error::Database(database) if database.is_unique_violation()) {
        RepositoryError::AlreadyExists
    } else {
        map_write(error)
    }
}

fn map_read(error: sqlx::Error) -> RepositoryError {
    match error {
        sqlx::Error::RowNotFound => RepositoryError::NotFound,
        sqlx::Error::ColumnDecode { .. }
        | sqlx::Error::ColumnNotFound(_)
        | sqlx::Error::Decode(_) => RepositoryError::Corrupt,
        other => map_transient(other),
    }
}

fn map_write(error: sqlx::Error) -> RepositoryError {
    if matches!(&error, sqlx::Error::Database(database) if database.is_unique_violation() || database.is_foreign_key_violation() || database.is_check_violation())
    {
        RepositoryError::Corrupt
    } else {
        map_transient(error)
    }
}

fn map_transient(_error: sqlx::Error) -> RepositoryError {
    RepositoryError::Unavailable
}
