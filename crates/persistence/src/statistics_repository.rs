use sqlx::{Row, Sqlite, Transaction, sqlite::SqliteRow};
use word_arena_application::{
    MatchStatisticsInput, OperatorStatistics, PublicStatistics, STATISTICS_SCHEMA_VERSION,
    StatisticsAccumulator, StatisticsFilter, StatisticsObservation, StatisticsRecordResult,
    StatisticsRepository, StatisticsRepositoryError, UnixMillis,
};

#[derive(Clone, Debug)]
pub struct SqliteStatisticsRepository {
    pool: sqlx::SqlitePool,
}

impl SqliteStatisticsRepository {
    #[must_use]
    pub const fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    async fn record_source(
        &self,
        source: MatchStatisticsInput,
        now: UnixMillis,
    ) -> Result<StatisticsRecordResult, StatisticsRepositoryError> {
        let derived = source
            .derive()
            .map_err(|_| StatisticsRepositoryError::InvalidInput)?;
        if now.0 < 0 {
            return Err(StatisticsRepositoryError::InvalidInput);
        }
        match self.load_source(&source.source_id).await {
            Ok((existing, observations)) if existing == source && observations == derived => {
                return Ok(StatisticsRecordResult::AlreadyApplied(observations));
            }
            Ok(_) => return Err(StatisticsRepositoryError::Conflict),
            Err(SourceLoadError::NotFound) => {}
            Err(SourceLoadError::Repository(error)) => return Err(error),
        }
        let source_json =
            serde_json::to_vec(&source).map_err(|_| StatisticsRepositoryError::Corrupt)?;
        let observations_json =
            serde_json::to_vec(&derived).map_err(|_| StatisticsRepositoryError::Corrupt)?;
        let mut transaction = self.pool.begin().await.map_err(map_storage)?;
        sqlx::query(
            "INSERT INTO statistics_sources (
                source_id, statistics_schema_version, tournament_id, match_id,
                game_id, finished_at_ms, source_json, observations_json, recorded_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&source.source_id)
        .bind(i64::from(source.schema_version))
        .bind(&source.tournament_id)
        .bind(&source.match_id)
        .bind(&source.game_id)
        .bind(source.finished_at.0)
        .bind(source_json)
        .bind(observations_json)
        .bind(now.0)
        .execute(&mut *transaction)
        .await
        .map_err(map_insert)?;
        for observation in &derived {
            insert_observation(&mut transaction, observation).await?;
        }
        transaction.commit().await.map_err(map_storage)?;
        Ok(StatisticsRecordResult::Applied(derived))
    }

    async fn load_source(
        &self,
        source_id: &str,
    ) -> Result<(MatchStatisticsInput, [StatisticsObservation; 2]), SourceLoadError> {
        validate_id(source_id).map_err(SourceLoadError::Repository)?;
        let row = sqlx::query("SELECT * FROM statistics_sources WHERE source_id = ?")
            .bind(source_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_storage)
            .map_err(SourceLoadError::Repository)?
            .ok_or(SourceLoadError::NotFound)?;
        let source = parse_source(&row).map_err(SourceLoadError::Repository)?;
        let observations = source
            .derive()
            .map_err(|_| SourceLoadError::Repository(StatisticsRepositoryError::Corrupt))?;
        let observation_bytes =
            bytes(&row, "observations_json").map_err(SourceLoadError::Repository)?;
        let stored = serde_json::from_slice::<[StatisticsObservation; 2]>(&observation_bytes)
            .map_err(|_| SourceLoadError::Repository(StatisticsRepositoryError::Corrupt))?;
        if stored != observations {
            return Err(SourceLoadError::Repository(
                StatisticsRepositoryError::Corrupt,
            ));
        }
        validate_observation_rows(&self.pool, source_id, &observations)
            .await
            .map_err(SourceLoadError::Repository)?;
        Ok((source, observations))
    }

    async fn rebuild(
        &self,
        filter: StatisticsFilter,
    ) -> Result<StatisticsAccumulator, StatisticsRepositoryError> {
        let mut accumulator = StatisticsAccumulator::new(filter)
            .map_err(|_| StatisticsRepositoryError::InvalidInput)?;
        let source_ids = sqlx::query_scalar::<_, String>(
            "SELECT source_id FROM statistics_sources ORDER BY source_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_storage)?;
        for source_id in source_ids {
            let (_, observations) =
                self.load_source(&source_id)
                    .await
                    .map_err(|error| match error {
                        SourceLoadError::NotFound => StatisticsRepositoryError::Corrupt,
                        SourceLoadError::Repository(error) => error,
                    })?;
            for observation in observations {
                accumulator
                    .add(observation)
                    .map_err(|_| StatisticsRepositoryError::Corrupt)?;
            }
        }
        Ok(accumulator)
    }
}

impl StatisticsRepository for SqliteStatisticsRepository {
    fn record(
        &self,
        source: MatchStatisticsInput,
        now: UnixMillis,
    ) -> word_arena_application::BoxFuture<
        '_,
        Result<StatisticsRecordResult, StatisticsRepositoryError>,
    > {
        Box::pin(self.record_source(source, now))
    }

    fn rebuild_public(
        &self,
        filter: StatisticsFilter,
    ) -> word_arena_application::BoxFuture<'_, Result<PublicStatistics, StatisticsRepositoryError>>
    {
        Box::pin(async move {
            self.rebuild(filter)
                .await?
                .public()
                .map_err(|_| StatisticsRepositoryError::Corrupt)
        })
    }

    fn rebuild_operator(
        &self,
        filter: StatisticsFilter,
    ) -> word_arena_application::BoxFuture<'_, Result<OperatorStatistics, StatisticsRepositoryError>>
    {
        Box::pin(async move {
            self.rebuild(filter)
                .await?
                .operator()
                .map_err(|_| StatisticsRepositoryError::Corrupt)
        })
    }
}

async fn insert_observation(
    transaction: &mut Transaction<'_, Sqlite>,
    observation: &StatisticsObservation,
) -> Result<(), StatisticsRepositoryError> {
    let source_id = observation
        .source_id
        .strip_suffix(&format!(":seat-{}", observation.scope.seat_number))
        .ok_or(StatisticsRepositoryError::Corrupt)?;
    let observation_json =
        serde_json::to_vec(observation).map_err(|_| StatisticsRepositoryError::Corrupt)?;
    sqlx::query(
        "INSERT INTO statistics_observations (
            observation_id, source_id, statistics_schema_version, language,
            ruleset_id, ruleset_sha256, pack_id, pack_version, pack_sha256,
            agent_manifest_sha256, tournament_id, match_id, game_id, entrant_id,
            seat_number, finished_at_ms, observation_json
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&observation.source_id)
    .bind(source_id)
    .bind(i64::from(observation.schema_version))
    .bind(&observation.scope.language)
    .bind(&observation.scope.ruleset_id)
    .bind(&observation.scope.ruleset_sha256)
    .bind(&observation.scope.pack_id)
    .bind(&observation.scope.pack_version)
    .bind(&observation.scope.pack_sha256)
    .bind(&observation.scope.agent_manifest_sha256)
    .bind(&observation.scope.tournament_id)
    .bind(&observation.scope.match_id)
    .bind(&observation.scope.game_id)
    .bind(&observation.scope.entrant_id)
    .bind(i64::from(observation.scope.seat_number))
    .bind(observation.scope.finished_at.0)
    .bind(observation_json)
    .execute(&mut **transaction)
    .await
    .map_err(map_insert)?;
    Ok(())
}

fn parse_source(row: &SqliteRow) -> Result<MatchStatisticsInput, StatisticsRepositoryError> {
    let source = serde_json::from_slice::<MatchStatisticsInput>(&bytes(row, "source_json")?)
        .map_err(|_| StatisticsRepositoryError::Corrupt)?;
    if text(row, "source_id")? != source.source_id
        || integer(row, "statistics_schema_version")? != i64::from(STATISTICS_SCHEMA_VERSION)
        || optional_text(row, "tournament_id")? != source.tournament_id
        || text(row, "match_id")? != source.match_id
        || text(row, "game_id")? != source.game_id
        || integer(row, "finished_at_ms")? != source.finished_at.0
        || integer(row, "recorded_at_ms")? < 0
    {
        return Err(StatisticsRepositoryError::Corrupt);
    }
    Ok(source)
}

async fn validate_observation_rows(
    pool: &sqlx::SqlitePool,
    source_id: &str,
    expected: &[StatisticsObservation; 2],
) -> Result<(), StatisticsRepositoryError> {
    let rows = sqlx::query(
        "SELECT * FROM statistics_observations
         WHERE source_id = ? ORDER BY seat_number",
    )
    .bind(source_id)
    .fetch_all(pool)
    .await
    .map_err(map_storage)?;
    if rows.len() != expected.len() {
        return Err(StatisticsRepositoryError::Corrupt);
    }
    for (row, observation) in rows.iter().zip(expected) {
        let parsed =
            serde_json::from_slice::<StatisticsObservation>(&bytes(row, "observation_json")?)
                .map_err(|_| StatisticsRepositoryError::Corrupt)?;
        let scope = &observation.scope;
        if parsed != *observation
            || text(row, "observation_id")? != observation.source_id
            || integer(row, "statistics_schema_version")? != i64::from(STATISTICS_SCHEMA_VERSION)
            || text(row, "language")? != scope.language
            || text(row, "ruleset_id")? != scope.ruleset_id
            || text(row, "ruleset_sha256")? != scope.ruleset_sha256
            || text(row, "pack_id")? != scope.pack_id
            || text(row, "pack_version")? != scope.pack_version
            || text(row, "pack_sha256")? != scope.pack_sha256
            || optional_text(row, "agent_manifest_sha256")? != scope.agent_manifest_sha256
            || optional_text(row, "tournament_id")? != scope.tournament_id
            || text(row, "match_id")? != scope.match_id
            || text(row, "game_id")? != scope.game_id
            || text(row, "entrant_id")? != scope.entrant_id
            || integer(row, "seat_number")? != i64::from(scope.seat_number)
            || integer(row, "finished_at_ms")? != scope.finished_at.0
        {
            return Err(StatisticsRepositoryError::Corrupt);
        }
    }
    Ok(())
}

enum SourceLoadError {
    NotFound,
    Repository(StatisticsRepositoryError),
}

fn bytes(row: &SqliteRow, column: &str) -> Result<Vec<u8>, StatisticsRepositoryError> {
    row.try_get(column)
        .map_err(|_| StatisticsRepositoryError::Corrupt)
}

fn text(row: &SqliteRow, column: &str) -> Result<String, StatisticsRepositoryError> {
    row.try_get(column)
        .map_err(|_| StatisticsRepositoryError::Corrupt)
}

fn optional_text(
    row: &SqliteRow,
    column: &str,
) -> Result<Option<String>, StatisticsRepositoryError> {
    row.try_get(column)
        .map_err(|_| StatisticsRepositoryError::Corrupt)
}

fn integer(row: &SqliteRow, column: &str) -> Result<i64, StatisticsRepositoryError> {
    row.try_get(column)
        .map_err(|_| StatisticsRepositoryError::Corrupt)
}

fn validate_id(value: &str) -> Result<(), StatisticsRepositoryError> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        Err(StatisticsRepositoryError::Corrupt)
    } else {
        Ok(())
    }
}

fn map_insert(error: sqlx::Error) -> StatisticsRepositoryError {
    if let sqlx::Error::Database(database) = &error
        && (database.is_unique_violation()
            || database.is_foreign_key_violation()
            || database.is_check_violation())
    {
        StatisticsRepositoryError::Conflict
    } else {
        map_storage(error)
    }
}

fn map_storage(_error: sqlx::Error) -> StatisticsRepositoryError {
    StatisticsRepositoryError::Unavailable
}
