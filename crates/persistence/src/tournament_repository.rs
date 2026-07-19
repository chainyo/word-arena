use sqlx::{Row, Sqlite, Transaction, sqlite::SqliteRow};
use word_arena_application::{
    BoxFuture, RepositoryError, ScheduledMatch, StoredTournament, TOURNAMENT_FORMAT_SCHEMA_VERSION,
    TOURNAMENT_LIFECYCLE_SCHEMA_VERSION, TOURNAMENT_SCHEDULE_SCHEMA_VERSION,
    TournamentLifecycleEvent, TournamentLifecycleState, TournamentRepository, UnixMillis,
};

#[derive(Clone, Debug)]
pub struct SqliteTournamentRepository {
    pool: sqlx::SqlitePool,
}

impl SqliteTournamentRepository {
    #[must_use]
    pub const fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    async fn insert_tournament(
        &self,
        tournament: &StoredTournament,
    ) -> Result<(), RepositoryError> {
        tournament
            .validate()
            .map_err(|_| RepositoryError::Corrupt)?;
        let initial = &tournament.lifecycle[0];
        let current = tournament
            .lifecycle
            .last()
            .ok_or(RepositoryError::Corrupt)?;
        let spec_json =
            serde_json::to_vec(&tournament.spec).map_err(|_| RepositoryError::Corrupt)?;
        let schedule_json =
            serde_json::to_vec(&tournament.schedule).map_err(|_| RepositoryError::Corrupt)?;
        let swiss_progress_json = tournament
            .swiss_progress
            .as_ref()
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|_| RepositoryError::Corrupt)?;
        let mut transaction = self.pool.begin().await.map_err(map_storage)?;
        sqlx::query(
            "INSERT INTO tournaments (
                tournament_id, schema_version, format_kind, status, config_json,
                created_at_ms, updated_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&tournament.spec.tournament_id)
        .bind(i64::from(TOURNAMENT_FORMAT_SCHEMA_VERSION))
        .bind(tournament.spec.format.kind())
        .bind(current.state.as_str())
        .bind(&spec_json)
        .bind(initial.occurred_at.0)
        .bind(current.occurred_at.0)
        .execute(&mut *transaction)
        .await
        .map_err(map_insert)?;
        insert_entries(&mut transaction, tournament).await?;
        sqlx::query(
            "INSERT INTO tournament_schedules (
                tournament_id, format_schema_version, schedule_schema_version,
                lifecycle_schema_version, format_identity_sha256, spec_json,
                schedule_json, swiss_progress_json, created_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&tournament.spec.tournament_id)
        .bind(i64::from(TOURNAMENT_FORMAT_SCHEMA_VERSION))
        .bind(i64::from(TOURNAMENT_SCHEDULE_SCHEMA_VERSION))
        .bind(i64::from(TOURNAMENT_LIFECYCLE_SCHEMA_VERSION))
        .bind(&tournament.schedule.format_identity.sha256)
        .bind(spec_json)
        .bind(schedule_json)
        .bind(swiss_progress_json)
        .bind(initial.occurred_at.0)
        .execute(&mut *transaction)
        .await
        .map_err(map_insert)?;
        insert_series_matches_and_byes(&mut transaction, tournament, initial.occurred_at).await?;
        for event in &tournament.lifecycle {
            insert_lifecycle(&mut transaction, &tournament.spec.tournament_id, event).await?;
        }
        transaction.commit().await.map_err(map_storage)
    }

    async fn load_tournament(
        &self,
        tournament_id: &str,
    ) -> Result<StoredTournament, RepositoryError> {
        validate_id(tournament_id)?;
        let row = sqlx::query(
            "SELECT t.schema_version, t.format_kind, t.status, t.config_json,
                    t.created_at_ms, t.updated_at_ms,
                    s.format_schema_version, s.schedule_schema_version,
                    s.lifecycle_schema_version, s.format_identity_sha256,
                    s.spec_json, s.schedule_json, s.swiss_progress_json,
                    s.created_at_ms AS schedule_created_at_ms
             FROM tournaments AS t
             JOIN tournament_schedules AS s USING (tournament_id)
             WHERE t.tournament_id = ?",
        )
        .bind(tournament_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_storage)?
        .ok_or(RepositoryError::NotFound)?;
        let config_json = bytes(&row, "config_json")?;
        let spec_json = bytes(&row, "spec_json")?;
        let schedule_json = bytes(&row, "schedule_json")?;
        let progress_json: Option<Vec<u8>> = row
            .try_get("swiss_progress_json")
            .map_err(|_| RepositoryError::Corrupt)?;
        if config_json != spec_json {
            return Err(RepositoryError::Corrupt);
        }
        let spec = serde_json::from_slice(&spec_json).map_err(|_| RepositoryError::Corrupt)?;
        let schedule =
            serde_json::from_slice(&schedule_json).map_err(|_| RepositoryError::Corrupt)?;
        let swiss_progress = progress_json
            .as_deref()
            .map(serde_json::from_slice)
            .transpose()
            .map_err(|_| RepositoryError::Corrupt)?;
        let lifecycle = load_lifecycle(&self.pool, tournament_id).await?;
        let tournament = StoredTournament {
            spec,
            schedule,
            swiss_progress,
            lifecycle,
        };
        tournament
            .validate()
            .map_err(|_| RepositoryError::Corrupt)?;
        validate_header(&row, &tournament)?;
        validate_entries(&self.pool, &tournament).await?;
        validate_series(&self.pool, &tournament).await?;
        validate_matches(&self.pool, &tournament).await?;
        validate_byes(&self.pool, &tournament).await?;
        Ok(tournament)
    }

    async fn transition_tournament(
        &self,
        tournament_id: &str,
        expected_sequence: u64,
        state: TournamentLifecycleState,
        occurred_at: UnixMillis,
    ) -> Result<TournamentLifecycleEvent, RepositoryError> {
        validate_id(tournament_id)?;
        if occurred_at.0 < 0 {
            return Err(RepositoryError::Corrupt);
        }
        let expected = i64::try_from(expected_sequence).map_err(|_| RepositoryError::Conflict)?;
        let mut transaction = self.pool.begin().await.map_err(map_storage)?;
        let row = sqlx::query(
            "SELECT sequence, state, occurred_at_ms
             FROM tournament_lifecycle_events
             WHERE tournament_id = ? ORDER BY sequence DESC LIMIT 1",
        )
        .bind(tournament_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_storage)?
        .ok_or(RepositoryError::NotFound)?;
        let current_sequence: i64 = row
            .try_get("sequence")
            .map_err(|_| RepositoryError::Corrupt)?;
        let current_state = lifecycle_state(
            row.try_get::<String, _>("state")
                .map_err(|_| RepositoryError::Corrupt)?
                .as_str(),
        )?;
        let previous_at: i64 = row
            .try_get("occurred_at_ms")
            .map_err(|_| RepositoryError::Corrupt)?;
        if current_sequence != expected
            || !current_state.can_transition_to(state)
            || occurred_at.0 < previous_at
        {
            return Err(RepositoryError::Conflict);
        }
        let sequence = expected_sequence
            .checked_add(1)
            .ok_or(RepositoryError::Conflict)?;
        let event = TournamentLifecycleEvent {
            schema_version: TOURNAMENT_LIFECYCLE_SCHEMA_VERSION,
            sequence,
            state,
            occurred_at,
        };
        insert_lifecycle(&mut transaction, tournament_id, &event).await?;
        let updated = sqlx::query(
            "UPDATE tournaments SET status = ?, updated_at_ms = ?
             WHERE tournament_id = ? AND status = ? AND updated_at_ms <= ?",
        )
        .bind(state.as_str())
        .bind(occurred_at.0)
        .bind(tournament_id)
        .bind(current_state.as_str())
        .bind(occurred_at.0)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage)?;
        if updated.rows_affected() != 1 {
            return Err(RepositoryError::Conflict);
        }
        transaction.commit().await.map_err(map_storage)?;
        Ok(event)
    }
}

impl TournamentRepository for SqliteTournamentRepository {
    fn insert(&self, tournament: StoredTournament) -> BoxFuture<'_, Result<(), RepositoryError>> {
        Box::pin(async move { self.insert_tournament(&tournament).await })
    }

    fn load<'a>(
        &'a self,
        tournament_id: &'a str,
    ) -> BoxFuture<'a, Result<StoredTournament, RepositoryError>> {
        Box::pin(self.load_tournament(tournament_id))
    }

    fn transition<'a>(
        &'a self,
        tournament_id: &'a str,
        expected_sequence: u64,
        state: TournamentLifecycleState,
        occurred_at: UnixMillis,
    ) -> BoxFuture<'a, Result<TournamentLifecycleEvent, RepositoryError>> {
        Box::pin(self.transition_tournament(tournament_id, expected_sequence, state, occurred_at))
    }
}

async fn insert_entries(
    transaction: &mut Transaction<'_, Sqlite>,
    tournament: &StoredTournament,
) -> Result<(), RepositoryError> {
    for entrant in &tournament.spec.entrants {
        sqlx::query(
            "INSERT INTO tournament_entries (
                tournament_id, entrant_id, seed_number, manifest_sha256
             ) VALUES (?, ?, ?, ?)",
        )
        .bind(&tournament.spec.tournament_id)
        .bind(&entrant.entrant_id)
        .bind(i64::from(entrant.seed_number))
        .bind(&entrant.manifest_sha256)
        .execute(&mut **transaction)
        .await
        .map_err(map_insert)?;
    }
    Ok(())
}

async fn insert_series_matches_and_byes(
    transaction: &mut Transaction<'_, Sqlite>,
    tournament: &StoredTournament,
    created_at: UnixMillis,
) -> Result<(), RepositoryError> {
    for series in &tournament.schedule.series {
        sqlx::query(
            "INSERT INTO tournament_series (
                series_id, tournament_id, round_number, table_number,
                entrant_a, entrant_b, match_count, status
             ) VALUES (?, ?, ?, ?, ?, ?, ?, 'pending')",
        )
        .bind(&series.series_id)
        .bind(&tournament.spec.tournament_id)
        .bind(i64::from(series.round_number))
        .bind(i64::from(series.table_number))
        .bind(&series.entrant_a)
        .bind(&series.entrant_b)
        .bind(i64::try_from(series.match_ids.len()).map_err(|_| RepositoryError::Corrupt)?)
        .execute(&mut **transaction)
        .await
        .map_err(map_insert)?;
    }
    for game in &tournament.schedule.matches {
        insert_match(transaction, tournament, game, created_at).await?;
    }
    for bye in &tournament.schedule.byes {
        sqlx::query(
            "INSERT INTO tournament_byes (tournament_id, round_number, entrant_id)
             VALUES (?, ?, ?)",
        )
        .bind(&tournament.spec.tournament_id)
        .bind(i64::from(bye.round_number))
        .bind(&bye.entrant_id)
        .execute(&mut **transaction)
        .await
        .map_err(map_insert)?;
    }
    Ok(())
}

async fn insert_match(
    transaction: &mut Transaction<'_, Sqlite>,
    tournament: &StoredTournament,
    game: &ScheduledMatch,
    created_at: UnixMillis,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "INSERT INTO matches (
            match_id, tournament_id, sequence, game_id, language,
            ruleset_id, ruleset_sha256, lexicon_pack_id,
            lexicon_pack_version, lexicon_content_sha256, status,
            scheduled_at_ms, started_at_ms, finished_at_ms, created_at_ms
         ) VALUES (?, ?, ?, NULL, ?, ?, ?, ?, ?, ?, 'pending', ?, NULL, NULL, ?)",
    )
    .bind(&game.match_id)
    .bind(&tournament.spec.tournament_id)
    .bind(i64::try_from(game.sequence).map_err(|_| RepositoryError::Corrupt)?)
    .bind(&game.profile.language)
    .bind(&game.profile.ruleset_id)
    .bind(&game.profile.ruleset_sha256)
    .bind(&game.profile.lexicon.pack_id)
    .bind(&game.profile.lexicon.pack_version)
    .bind(&game.profile.lexicon.content_sha256)
    .bind(created_at.0)
    .bind(created_at.0)
    .execute(&mut **transaction)
    .await
    .map_err(map_insert)?;
    sqlx::query(
        "INSERT INTO tournament_match_schedule (
            match_id, tournament_id, series_id, sequence, round_number,
            table_number, series_game_number, game_seed_commitment_sha256,
            format_identity_sha256
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&game.match_id)
    .bind(&tournament.spec.tournament_id)
    .bind(&game.series_id)
    .bind(i64::try_from(game.sequence).map_err(|_| RepositoryError::Corrupt)?)
    .bind(i64::from(game.round_number))
    .bind(i64::from(game.table_number))
    .bind(i64::from(game.series_game_number))
    .bind(&game.game_seed_commitment_sha256)
    .bind(&tournament.schedule.format_identity.sha256)
    .execute(&mut **transaction)
    .await
    .map_err(map_insert)?;
    for (seat, entrant) in [
        (1_i64, &game.seat_one_entrant_id),
        (2_i64, &game.seat_two_entrant_id),
    ] {
        sqlx::query(
            "INSERT INTO tournament_match_seats (
                match_id, tournament_id, seat_number, entrant_id
             ) VALUES (?, ?, ?, ?)",
        )
        .bind(&game.match_id)
        .bind(&tournament.spec.tournament_id)
        .bind(seat)
        .bind(entrant)
        .execute(&mut **transaction)
        .await
        .map_err(map_insert)?;
    }
    Ok(())
}

async fn insert_lifecycle(
    transaction: &mut Transaction<'_, Sqlite>,
    tournament_id: &str,
    event: &TournamentLifecycleEvent,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "INSERT INTO tournament_lifecycle_events (
            tournament_id, sequence, lifecycle_schema_version, state, occurred_at_ms
         ) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(tournament_id)
    .bind(i64::try_from(event.sequence).map_err(|_| RepositoryError::Corrupt)?)
    .bind(i64::from(event.schema_version))
    .bind(event.state.as_str())
    .bind(event.occurred_at.0)
    .execute(&mut **transaction)
    .await
    .map_err(map_insert)?;
    Ok(())
}

async fn load_lifecycle(
    pool: &sqlx::SqlitePool,
    tournament_id: &str,
) -> Result<Vec<TournamentLifecycleEvent>, RepositoryError> {
    sqlx::query(
        "SELECT sequence, lifecycle_schema_version, state, occurred_at_ms
         FROM tournament_lifecycle_events
         WHERE tournament_id = ? ORDER BY sequence",
    )
    .bind(tournament_id)
    .fetch_all(pool)
    .await
    .map_err(map_storage)?
    .into_iter()
    .map(|row| {
        Ok(TournamentLifecycleEvent {
            schema_version: u32::try_from(integer(&row, "lifecycle_schema_version")?)
                .map_err(|_| RepositoryError::Corrupt)?,
            sequence: u64::try_from(integer(&row, "sequence")?)
                .map_err(|_| RepositoryError::Corrupt)?,
            state: lifecycle_state(text(&row, "state")?.as_str())?,
            occurred_at: UnixMillis(integer(&row, "occurred_at_ms")?),
        })
    })
    .collect()
}

fn validate_header(row: &SqliteRow, tournament: &StoredTournament) -> Result<(), RepositoryError> {
    let first = &tournament.lifecycle[0];
    let last = tournament
        .lifecycle
        .last()
        .ok_or(RepositoryError::Corrupt)?;
    if integer(row, "schema_version")? != i64::from(TOURNAMENT_FORMAT_SCHEMA_VERSION)
        || integer(row, "format_schema_version")? != i64::from(TOURNAMENT_FORMAT_SCHEMA_VERSION)
        || integer(row, "schedule_schema_version")? != i64::from(TOURNAMENT_SCHEDULE_SCHEMA_VERSION)
        || integer(row, "lifecycle_schema_version")?
            != i64::from(TOURNAMENT_LIFECYCLE_SCHEMA_VERSION)
        || text(row, "format_kind")? != tournament.spec.format.kind()
        || text(row, "status")? != last.state.as_str()
        || text(row, "format_identity_sha256")? != tournament.schedule.format_identity.sha256
        || integer(row, "created_at_ms")? != first.occurred_at.0
        || integer(row, "schedule_created_at_ms")? != first.occurred_at.0
        || integer(row, "updated_at_ms")? != last.occurred_at.0
    {
        return Err(RepositoryError::Corrupt);
    }
    Ok(())
}

async fn validate_entries(
    pool: &sqlx::SqlitePool,
    tournament: &StoredTournament,
) -> Result<(), RepositoryError> {
    let rows = sqlx::query(
        "SELECT entrant_id, seed_number, manifest_sha256 FROM tournament_entries
         WHERE tournament_id = ? ORDER BY seed_number, entrant_id",
    )
    .bind(&tournament.spec.tournament_id)
    .fetch_all(pool)
    .await
    .map_err(map_storage)?;
    let mut expected = tournament.spec.entrants.clone();
    expected.sort_by(|left, right| {
        left.seed_number
            .cmp(&right.seed_number)
            .then_with(|| left.entrant_id.cmp(&right.entrant_id))
    });
    if rows.len() != expected.len() {
        return Err(RepositoryError::Corrupt);
    }
    for (row, entrant) in rows.iter().zip(expected) {
        if text(row, "entrant_id")? != entrant.entrant_id
            || integer(row, "seed_number")? != i64::from(entrant.seed_number)
            || row
                .try_get::<Option<String>, _>("manifest_sha256")
                .map_err(|_| RepositoryError::Corrupt)?
                != entrant.manifest_sha256
        {
            return Err(RepositoryError::Corrupt);
        }
    }
    Ok(())
}

async fn validate_series(
    pool: &sqlx::SqlitePool,
    tournament: &StoredTournament,
) -> Result<(), RepositoryError> {
    let rows = sqlx::query(
        "SELECT series_id, round_number, table_number, entrant_a, entrant_b,
                match_count, status
         FROM tournament_series WHERE tournament_id = ?
         ORDER BY round_number, table_number",
    )
    .bind(&tournament.spec.tournament_id)
    .fetch_all(pool)
    .await
    .map_err(map_storage)?;
    if rows.len() != tournament.schedule.series.len() {
        return Err(RepositoryError::Corrupt);
    }
    for (row, series) in rows.iter().zip(&tournament.schedule.series) {
        if text(row, "series_id")? != series.series_id
            || integer(row, "round_number")? != i64::from(series.round_number)
            || integer(row, "table_number")? != i64::from(series.table_number)
            || text(row, "entrant_a")? != series.entrant_a
            || text(row, "entrant_b")? != series.entrant_b
            || integer(row, "match_count")?
                != i64::try_from(series.match_ids.len()).map_err(|_| RepositoryError::Corrupt)?
            || text(row, "status")? != "pending"
        {
            return Err(RepositoryError::Corrupt);
        }
    }
    Ok(())
}

async fn validate_matches(
    pool: &sqlx::SqlitePool,
    tournament: &StoredTournament,
) -> Result<(), RepositoryError> {
    let rows = sqlx::query(
        "SELECT ms.match_id, ms.series_id, ms.sequence, ms.round_number,
                ms.table_number, ms.series_game_number,
                ms.game_seed_commitment_sha256, ms.format_identity_sha256,
                m.language, m.ruleset_id, m.ruleset_sha256,
                m.lexicon_pack_id, m.lexicon_pack_version,
                m.lexicon_content_sha256, m.status,
                MAX(CASE WHEN seats.seat_number = 1 THEN seats.entrant_id END) AS seat_one,
                MAX(CASE WHEN seats.seat_number = 2 THEN seats.entrant_id END) AS seat_two,
                COUNT(seats.seat_number) AS seat_count
         FROM tournament_match_schedule AS ms
         JOIN matches AS m ON m.match_id = ms.match_id
         JOIN tournament_match_seats AS seats ON seats.match_id = ms.match_id
         WHERE ms.tournament_id = ?
         GROUP BY ms.match_id ORDER BY ms.sequence",
    )
    .bind(&tournament.spec.tournament_id)
    .fetch_all(pool)
    .await
    .map_err(map_storage)?;
    if rows.len() != tournament.schedule.matches.len() {
        return Err(RepositoryError::Corrupt);
    }
    for (row, game) in rows.iter().zip(&tournament.schedule.matches) {
        validate_match_row(row, game, &tournament.schedule.format_identity.sha256)?;
    }
    Ok(())
}

fn validate_match_row(
    row: &SqliteRow,
    game: &ScheduledMatch,
    format_sha256: &str,
) -> Result<(), RepositoryError> {
    if text(row, "match_id")? != game.match_id
        || text(row, "series_id")? != game.series_id
        || integer(row, "sequence")?
            != i64::try_from(game.sequence).map_err(|_| RepositoryError::Corrupt)?
        || integer(row, "round_number")? != i64::from(game.round_number)
        || integer(row, "table_number")? != i64::from(game.table_number)
        || integer(row, "series_game_number")? != i64::from(game.series_game_number)
        || text(row, "game_seed_commitment_sha256")? != game.game_seed_commitment_sha256
        || text(row, "format_identity_sha256")? != format_sha256
        || text(row, "language")? != game.profile.language
        || text(row, "ruleset_id")? != game.profile.ruleset_id
        || text(row, "ruleset_sha256")? != game.profile.ruleset_sha256
        || text(row, "lexicon_pack_id")? != game.profile.lexicon.pack_id
        || text(row, "lexicon_pack_version")? != game.profile.lexicon.pack_version
        || text(row, "lexicon_content_sha256")? != game.profile.lexicon.content_sha256
        || text(row, "status")? != "pending"
        || text(row, "seat_one")? != game.seat_one_entrant_id
        || text(row, "seat_two")? != game.seat_two_entrant_id
        || integer(row, "seat_count")? != 2
    {
        return Err(RepositoryError::Corrupt);
    }
    Ok(())
}

async fn validate_byes(
    pool: &sqlx::SqlitePool,
    tournament: &StoredTournament,
) -> Result<(), RepositoryError> {
    let rows = sqlx::query(
        "SELECT round_number, entrant_id FROM tournament_byes
         WHERE tournament_id = ? ORDER BY round_number",
    )
    .bind(&tournament.spec.tournament_id)
    .fetch_all(pool)
    .await
    .map_err(map_storage)?;
    if rows.len() != tournament.schedule.byes.len() {
        return Err(RepositoryError::Corrupt);
    }
    for (row, bye) in rows.iter().zip(&tournament.schedule.byes) {
        if integer(row, "round_number")? != i64::from(bye.round_number)
            || text(row, "entrant_id")? != bye.entrant_id
        {
            return Err(RepositoryError::Corrupt);
        }
    }
    Ok(())
}

fn lifecycle_state(value: &str) -> Result<TournamentLifecycleState, RepositoryError> {
    match value {
        "draft" => Ok(TournamentLifecycleState::Draft),
        "scheduled" => Ok(TournamentLifecycleState::Scheduled),
        "running" => Ok(TournamentLifecycleState::Running),
        "paused" => Ok(TournamentLifecycleState::Paused),
        "finished" => Ok(TournamentLifecycleState::Finished),
        "cancelled" => Ok(TournamentLifecycleState::Cancelled),
        _ => Err(RepositoryError::Corrupt),
    }
}

fn bytes(row: &SqliteRow, column: &str) -> Result<Vec<u8>, RepositoryError> {
    row.try_get(column).map_err(|_| RepositoryError::Corrupt)
}

fn text(row: &SqliteRow, column: &str) -> Result<String, RepositoryError> {
    row.try_get(column).map_err(|_| RepositoryError::Corrupt)
}

fn integer(row: &SqliteRow, column: &str) -> Result<i64, RepositoryError> {
    row.try_get(column).map_err(|_| RepositoryError::Corrupt)
}

fn validate_id(value: &str) -> Result<(), RepositoryError> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        Err(RepositoryError::Corrupt)
    } else {
        Ok(())
    }
}

fn map_insert(error: sqlx::Error) -> RepositoryError {
    if let sqlx::Error::Database(database) = &error {
        if database.is_unique_violation() {
            return RepositoryError::AlreadyExists;
        }
        if database.is_foreign_key_violation() || database.is_check_violation() {
            return RepositoryError::Conflict;
        }
    }
    map_storage(error)
}

fn map_storage(_error: sqlx::Error) -> RepositoryError {
    RepositoryError::Unavailable
}
