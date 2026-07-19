use std::collections::BTreeMap;

use sqlx::{Row, Sqlite, Transaction, sqlite::SqliteRow};
use word_arena_application::{
    RATING_SCHEMA_VERSION, RatingCommitResult, RatingOpponent, RatingPeriod, RatingPool,
    RatingRepository, RatingRepositoryError, RatingValue, StoredRatingPeriod, UnixMillis,
};

#[derive(Clone, Debug)]
pub struct SqliteRatingRepository {
    pool: sqlx::SqlitePool,
}

impl SqliteRatingRepository {
    #[must_use]
    pub const fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    async fn commit_period(
        &self,
        period: RatingPeriod,
        now: UnixMillis,
    ) -> Result<RatingCommitResult, RatingRepositoryError> {
        period
            .validate()
            .map_err(|_| RatingRepositoryError::Corrupt)?;
        if now.0 < 0 {
            return Err(RatingRepositoryError::Corrupt);
        }
        let derived = period
            .derive()
            .map_err(|_| RatingRepositoryError::Corrupt)?;
        match self.load_period(&period.period_id).await {
            Ok(existing) if existing.period == period && existing.derived == derived => {
                return Ok(RatingCommitResult::AlreadyApplied(existing));
            }
            Ok(_) => return Err(RatingRepositoryError::Conflict),
            Err(RatingRepositoryError::NotFound) => {}
            Err(error) => return Err(error),
        }
        let pool_key = period
            .pool
            .key()
            .map_err(|_| RatingRepositoryError::Corrupt)?;
        let mut transaction = self.pool.begin().await.map_err(map_storage)?;
        validate_sequence(&mut transaction, &pool_key, period.sequence).await?;
        validate_previous(&mut transaction, &pool_key, &period).await?;
        let pool_json =
            serde_json::to_vec(&period.pool).map_err(|_| RatingRepositoryError::Corrupt)?;
        let period_json =
            serde_json::to_vec(&period).map_err(|_| RatingRepositoryError::Corrupt)?;
        let derived_json =
            serde_json::to_vec(&derived).map_err(|_| RatingRepositoryError::Corrupt)?;
        sqlx::query(
            "INSERT INTO rating_periods (
                period_id, schema_version, pool_key, sequence, pool_json,
                period_json, derived_json, committed_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&period.period_id)
        .bind(i64::from(period.schema_version))
        .bind(&pool_key)
        .bind(i64::try_from(period.sequence).map_err(|_| RatingRepositoryError::Corrupt)?)
        .bind(pool_json)
        .bind(period_json)
        .bind(derived_json)
        .bind(now.0)
        .execute(&mut *transaction)
        .await
        .map_err(map_insert)?;
        insert_matches(&mut transaction, &pool_key, &period).await?;
        insert_updates(&mut transaction, &pool_key, &period, &derived).await?;
        transaction.commit().await.map_err(map_storage)?;
        Ok(RatingCommitResult::Applied(StoredRatingPeriod {
            period,
            derived,
            committed_at: now,
        }))
    }

    async fn load_period(
        &self,
        period_id: &str,
    ) -> Result<StoredRatingPeriod, RatingRepositoryError> {
        validate_id(period_id)?;
        let row = sqlx::query("SELECT * FROM rating_periods WHERE period_id = ?")
            .bind(period_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_storage)?
            .ok_or(RatingRepositoryError::NotFound)?;
        let stored = stored_period(&row)?;
        validate_normalized_rows(&self.pool, &stored).await?;
        Ok(stored)
    }

    async fn rebuild_pool(
        &self,
        pool: RatingPool,
    ) -> Result<Vec<(String, RatingValue)>, RatingRepositoryError> {
        let pool_key = pool.key().map_err(|_| RatingRepositoryError::Corrupt)?;
        let rows = sqlx::query(
            "SELECT * FROM rating_periods WHERE pool_key = ? ORDER BY sequence, period_id",
        )
        .bind(&pool_key)
        .fetch_all(&self.pool)
        .await
        .map_err(map_storage)?;
        let mut ratings = BTreeMap::new();
        for (expected_sequence, row) in rows.iter().enumerate() {
            let stored = stored_period(row)?;
            validate_normalized_rows(&self.pool, &stored).await?;
            if stored.period.pool != pool
                || stored.period.sequence
                    != u64::try_from(expected_sequence)
                        .map_err(|_| RatingRepositoryError::Corrupt)?
            {
                return Err(RatingRepositoryError::Corrupt);
            }
            for input in &stored.period.updates {
                if ratings
                    .get(&input.entrant_id)
                    .is_some_and(|current| *current != input.previous)
                {
                    return Err(RatingRepositoryError::Corrupt);
                }
            }
            ratings.extend(stored.derived);
        }
        let rebuilt = ratings.into_iter().collect::<Vec<_>>();
        validate_current_rows(&self.pool, &pool_key, &rebuilt).await?;
        Ok(rebuilt)
    }
}

impl RatingRepository for SqliteRatingRepository {
    fn commit(
        &self,
        period: RatingPeriod,
        now: UnixMillis,
    ) -> word_arena_application::BoxFuture<'_, Result<RatingCommitResult, RatingRepositoryError>>
    {
        Box::pin(self.commit_period(period, now))
    }

    fn load<'a>(
        &'a self,
        period_id: &'a str,
    ) -> word_arena_application::BoxFuture<'a, Result<StoredRatingPeriod, RatingRepositoryError>>
    {
        Box::pin(self.load_period(period_id))
    }

    fn rebuild(
        &self,
        pool: RatingPool,
    ) -> word_arena_application::BoxFuture<
        '_,
        Result<Vec<(String, RatingValue)>, RatingRepositoryError>,
    > {
        Box::pin(self.rebuild_pool(pool))
    }
}

async fn validate_sequence(
    transaction: &mut Transaction<'_, Sqlite>,
    pool_key: &str,
    sequence: u64,
) -> Result<(), RatingRepositoryError> {
    let previous = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MAX(sequence) FROM rating_periods WHERE pool_key = ?",
    )
    .bind(pool_key)
    .fetch_one(&mut **transaction)
    .await
    .map_err(map_storage)?;
    let expected = previous.map_or(0, |value| value.saturating_add(1));
    if i64::try_from(sequence).map_err(|_| RatingRepositoryError::Corrupt)? == expected {
        Ok(())
    } else {
        Err(RatingRepositoryError::Conflict)
    }
}

async fn validate_previous(
    transaction: &mut Transaction<'_, Sqlite>,
    pool_key: &str,
    period: &RatingPeriod,
) -> Result<(), RatingRepositoryError> {
    for input in &period.updates {
        let row = sqlx::query(
            "SELECT rating_milli, deviation_milli, volatility_nano
             FROM current_ratings WHERE pool_key = ? AND entrant_id = ?",
        )
        .bind(pool_key)
        .bind(&input.entrant_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_storage)?;
        if let Some(row) = row
            && rating(&row)? != input.previous
        {
            return Err(RatingRepositoryError::Conflict);
        }
    }
    Ok(())
}

async fn insert_matches(
    transaction: &mut Transaction<'_, Sqlite>,
    pool_key: &str,
    period: &RatingPeriod,
) -> Result<(), RatingRepositoryError> {
    for game in &period.matches {
        sqlx::query(
            "INSERT INTO rating_match_inputs (
                pool_key, match_id, period_id, series_id, series_game_number,
                entrant_one, entrant_two, score_one_millionths
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(pool_key)
        .bind(&game.match_id)
        .bind(&period.period_id)
        .bind(&game.series_id)
        .bind(i64::from(game.series_game_number))
        .bind(&game.entrant_one)
        .bind(&game.entrant_two)
        .bind(i64::from(game.score_one_millionths))
        .execute(&mut **transaction)
        .await
        .map_err(map_insert)?;
    }
    Ok(())
}

async fn insert_updates(
    transaction: &mut Transaction<'_, Sqlite>,
    pool_key: &str,
    period: &RatingPeriod,
    derived: &[(String, RatingValue)],
) -> Result<(), RatingRepositoryError> {
    let derived = derived.iter().cloned().collect::<BTreeMap<_, _>>();
    for input in &period.updates {
        let opponents =
            serde_json::to_vec(&input.opponents).map_err(|_| RatingRepositoryError::Corrupt)?;
        sqlx::query(
            "INSERT INTO rating_period_inputs (
                period_id, entrant_id, previous_rating_milli, previous_deviation_milli,
                previous_volatility_nano, opponents_json
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&period.period_id)
        .bind(&input.entrant_id)
        .bind(input.previous.rating_milli)
        .bind(i64::from(input.previous.deviation_milli))
        .bind(i64::from(input.previous.volatility_nano))
        .bind(opponents)
        .execute(&mut **transaction)
        .await
        .map_err(map_insert)?;
        let next = derived
            .get(&input.entrant_id)
            .ok_or(RatingRepositoryError::Corrupt)?;
        sqlx::query(
            "INSERT INTO rating_updates (period_id, entrant_id, pool_key, sequence,
                rating_milli, deviation_milli, volatility_nano) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&period.period_id)
        .bind(&input.entrant_id)
        .bind(pool_key)
        .bind(i64::try_from(period.sequence).map_err(|_| RatingRepositoryError::Corrupt)?)
        .bind(next.rating_milli)
        .bind(i64::from(next.deviation_milli))
        .bind(i64::from(next.volatility_nano))
        .execute(&mut **transaction)
        .await
        .map_err(map_insert)?;
        sqlx::query(
            "INSERT INTO current_ratings (pool_key, entrant_id, period_id, sequence,
                rating_milli, deviation_milli, volatility_nano) VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(pool_key, entrant_id) DO UPDATE SET period_id = excluded.period_id,
                sequence = excluded.sequence, rating_milli = excluded.rating_milli,
                deviation_milli = excluded.deviation_milli,
                volatility_nano = excluded.volatility_nano
             WHERE current_ratings.sequence < excluded.sequence",
        )
        .bind(pool_key)
        .bind(&input.entrant_id)
        .bind(&period.period_id)
        .bind(i64::try_from(period.sequence).map_err(|_| RatingRepositoryError::Corrupt)?)
        .bind(next.rating_milli)
        .bind(i64::from(next.deviation_milli))
        .bind(i64::from(next.volatility_nano))
        .execute(&mut **transaction)
        .await
        .map_err(map_insert)?;
    }
    Ok(())
}

fn stored_period(row: &SqliteRow) -> Result<StoredRatingPeriod, RatingRepositoryError> {
    let period: RatingPeriod = serde_json::from_slice(&bytes(row, "period_json")?)
        .map_err(|_| RatingRepositoryError::Corrupt)?;
    let derived: Vec<(String, RatingValue)> = serde_json::from_slice(&bytes(row, "derived_json")?)
        .map_err(|_| RatingRepositoryError::Corrupt)?;
    period
        .validate()
        .map_err(|_| RatingRepositoryError::Corrupt)?;
    if period
        .derive()
        .map_err(|_| RatingRepositoryError::Corrupt)?
        != derived
        || text(row, "period_id")? != period.period_id
        || integer(row, "schema_version")? != i64::from(RATING_SCHEMA_VERSION)
        || integer(row, "sequence")?
            != i64::try_from(period.sequence).map_err(|_| RatingRepositoryError::Corrupt)?
        || text(row, "pool_key")?
            != period
                .pool
                .key()
                .map_err(|_| RatingRepositoryError::Corrupt)?
        || serde_json::from_slice::<RatingPool>(&bytes(row, "pool_json")?)
            .map_err(|_| RatingRepositoryError::Corrupt)?
            != period.pool
    {
        return Err(RatingRepositoryError::Corrupt);
    }
    Ok(StoredRatingPeriod {
        period,
        derived,
        committed_at: UnixMillis(integer(row, "committed_at_ms")?),
    })
}

async fn validate_current_rows(
    pool: &sqlx::SqlitePool,
    pool_key: &str,
    rebuilt: &[(String, RatingValue)],
) -> Result<(), RatingRepositoryError> {
    let rows = sqlx::query("SELECT entrant_id, rating_milli, deviation_milli, volatility_nano FROM current_ratings WHERE pool_key = ? ORDER BY entrant_id")
        .bind(pool_key).fetch_all(pool).await.map_err(map_storage)?;
    if rows.len() != rebuilt.len() {
        return Err(RatingRepositoryError::Corrupt);
    }
    for (row, (entrant, expected)) in rows.iter().zip(rebuilt) {
        if text(row, "entrant_id")? != *entrant || rating(row)? != *expected {
            return Err(RatingRepositoryError::Corrupt);
        }
    }
    Ok(())
}

async fn validate_normalized_rows(
    pool: &sqlx::SqlitePool,
    stored: &StoredRatingPeriod,
) -> Result<(), RatingRepositoryError> {
    let pool_key = stored
        .period
        .pool
        .key()
        .map_err(|_| RatingRepositoryError::Corrupt)?;
    let match_rows = sqlx::query(
        "SELECT pool_key, match_id, series_id, series_game_number, entrant_one,
                entrant_two, score_one_millionths
         FROM rating_match_inputs WHERE period_id = ? ORDER BY match_id",
    )
    .bind(&stored.period.period_id)
    .fetch_all(pool)
    .await
    .map_err(map_storage)?;
    let mut expected_matches = stored.period.matches.clone();
    expected_matches.sort_by(|left, right| left.match_id.cmp(&right.match_id));
    if match_rows.len() != expected_matches.len() {
        return Err(RatingRepositoryError::Corrupt);
    }
    for (row, expected) in match_rows.iter().zip(&expected_matches) {
        if text(row, "pool_key")? != pool_key
            || text(row, "match_id")? != expected.match_id
            || text(row, "series_id")? != expected.series_id
            || integer(row, "series_game_number")? != i64::from(expected.series_game_number)
            || text(row, "entrant_one")? != expected.entrant_one
            || text(row, "entrant_two")? != expected.entrant_two
            || integer(row, "score_one_millionths")? != i64::from(expected.score_one_millionths)
        {
            return Err(RatingRepositoryError::Corrupt);
        }
    }

    let input_rows = sqlx::query(
        "SELECT entrant_id, previous_rating_milli, previous_deviation_milli,
                previous_volatility_nano, opponents_json
         FROM rating_period_inputs WHERE period_id = ? ORDER BY entrant_id",
    )
    .bind(&stored.period.period_id)
    .fetch_all(pool)
    .await
    .map_err(map_storage)?;
    let mut expected_inputs = stored.period.updates.clone();
    expected_inputs.sort_by(|left, right| left.entrant_id.cmp(&right.entrant_id));
    if input_rows.len() != expected_inputs.len() {
        return Err(RatingRepositoryError::Corrupt);
    }
    for (row, expected) in input_rows.iter().zip(&expected_inputs) {
        let opponents =
            serde_json::from_slice::<Vec<RatingOpponent>>(&bytes(row, "opponents_json")?)
                .map_err(|_| RatingRepositoryError::Corrupt)?;
        if text(row, "entrant_id")? != expected.entrant_id
            || rating_with_prefix(row, "previous_")? != expected.previous
            || opponents != expected.opponents
        {
            return Err(RatingRepositoryError::Corrupt);
        }
    }

    let update_rows = sqlx::query(
        "SELECT entrant_id, pool_key, sequence, rating_milli, deviation_milli,
                volatility_nano
         FROM rating_updates WHERE period_id = ? ORDER BY entrant_id",
    )
    .bind(&stored.period.period_id)
    .fetch_all(pool)
    .await
    .map_err(map_storage)?;
    if update_rows.len() != stored.derived.len() {
        return Err(RatingRepositoryError::Corrupt);
    }
    for (row, (entrant_id, expected)) in update_rows.iter().zip(&stored.derived) {
        if text(row, "entrant_id")? != *entrant_id
            || text(row, "pool_key")? != pool_key
            || integer(row, "sequence")?
                != i64::try_from(stored.period.sequence)
                    .map_err(|_| RatingRepositoryError::Corrupt)?
            || rating(row)? != *expected
        {
            return Err(RatingRepositoryError::Corrupt);
        }
    }
    Ok(())
}

fn rating(row: &SqliteRow) -> Result<RatingValue, RatingRepositoryError> {
    Ok(RatingValue {
        rating_milli: row
            .try_get("rating_milli")
            .map_err(|_| RatingRepositoryError::Corrupt)?,
        deviation_milli: u32::try_from(integer(row, "deviation_milli")?)
            .map_err(|_| RatingRepositoryError::Corrupt)?,
        volatility_nano: u32::try_from(integer(row, "volatility_nano")?)
            .map_err(|_| RatingRepositoryError::Corrupt)?,
    })
}

fn rating_with_prefix(row: &SqliteRow, prefix: &str) -> Result<RatingValue, RatingRepositoryError> {
    Ok(RatingValue {
        rating_milli: row
            .try_get(format!("{prefix}rating_milli").as_str())
            .map_err(|_| RatingRepositoryError::Corrupt)?,
        deviation_milli: u32::try_from(integer(row, &format!("{prefix}deviation_milli"))?)
            .map_err(|_| RatingRepositoryError::Corrupt)?,
        volatility_nano: u32::try_from(integer(row, &format!("{prefix}volatility_nano"))?)
            .map_err(|_| RatingRepositoryError::Corrupt)?,
    })
}
fn bytes(row: &SqliteRow, column: &str) -> Result<Vec<u8>, RatingRepositoryError> {
    row.try_get(column)
        .map_err(|_| RatingRepositoryError::Corrupt)
}
fn text(row: &SqliteRow, column: &str) -> Result<String, RatingRepositoryError> {
    row.try_get(column)
        .map_err(|_| RatingRepositoryError::Corrupt)
}
fn integer(row: &SqliteRow, column: &str) -> Result<i64, RatingRepositoryError> {
    row.try_get(column)
        .map_err(|_| RatingRepositoryError::Corrupt)
}
fn validate_id(value: &str) -> Result<(), RatingRepositoryError> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        Err(RatingRepositoryError::Corrupt)
    } else {
        Ok(())
    }
}
fn map_insert(error: sqlx::Error) -> RatingRepositoryError {
    if let sqlx::Error::Database(database) = &error
        && (database.is_unique_violation()
            || database.is_foreign_key_violation()
            || database.is_check_violation())
    {
        RatingRepositoryError::Conflict
    } else {
        map_storage(error)
    }
}
fn map_storage(_error: sqlx::Error) -> RatingRepositoryError {
    RatingRepositoryError::Unavailable
}
