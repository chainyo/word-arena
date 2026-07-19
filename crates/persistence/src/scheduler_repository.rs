use std::collections::BTreeMap;

use sqlx::{Row, Sqlite, Transaction, sqlite::SqliteRow};
use word_arena_application::{
    ExecutionReservation, RatePolicy, RecoverySnapshot, ReservationRequest, ReservationResult,
    SCHEDULER_SCHEMA_VERSION, SchedulerRepository, SchedulerRepositoryError, SchedulerScope,
    SchedulingLimit, TerminalCommitResult, TerminalMatchResult, TokenBucketState,
    TournamentWorkerControl, UnixMillis, refill_bucket, token_retry_at,
};

#[derive(Clone, Debug)]
pub struct SqliteSchedulerRepository {
    pool: sqlx::SqlitePool,
}

#[derive(Debug)]
struct StoredLimit {
    policy: SchedulingLimit,
    bucket: Option<TokenBucketState>,
}

impl SqliteSchedulerRepository {
    #[must_use]
    pub const fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    async fn configure_limits(
        &self,
        limits: Vec<SchedulingLimit>,
        now: UnixMillis,
    ) -> Result<(), SchedulerRepositoryError> {
        if limits.is_empty() || now.0 < 0 {
            return Err(SchedulerRepositoryError::Corrupt);
        }
        let mut unique = BTreeMap::new();
        for limit in limits {
            limit
                .validate()
                .map_err(|_| SchedulerRepositoryError::Corrupt)?;
            if unique.insert(limit.scope.key(), limit).is_some() {
                return Err(SchedulerRepositoryError::Conflict);
            }
        }
        let mut transaction = self.pool.begin().await.map_err(map_storage)?;
        for (key, limit) in unique {
            let active = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM execution_reservation_scopes AS scopes
                 JOIN execution_reservations AS reservations USING (reservation_id)
                 WHERE scopes.scope_key = ? AND reservations.status IN ('active', 'cancel_requested')",
            )
            .bind(&key)
            .fetch_one(&mut *transaction)
            .await
            .map_err(map_storage)?;
            if active != 0 {
                return Err(SchedulerRepositoryError::Conflict);
            }
            let scope_json =
                serde_json::to_vec(&limit.scope).map_err(|_| SchedulerRepositoryError::Corrupt)?;
            let existing_created = sqlx::query_scalar::<_, i64>(
                "SELECT created_at_ms FROM scheduler_limits WHERE scope_key = ?",
            )
            .bind(&key)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(map_storage)?
            .unwrap_or(now.0);
            sqlx::query(
                "INSERT INTO scheduler_limits (
                    scope_key, schema_version, scope_json, max_concurrency,
                    rate_capacity, refill_tokens, refill_interval_ms, created_at_ms, updated_at_ms
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(scope_key) DO UPDATE SET
                    schema_version = excluded.schema_version,
                    scope_json = excluded.scope_json,
                    max_concurrency = excluded.max_concurrency,
                    rate_capacity = excluded.rate_capacity,
                    refill_tokens = excluded.refill_tokens,
                    refill_interval_ms = excluded.refill_interval_ms,
                    updated_at_ms = excluded.updated_at_ms",
            )
            .bind(&key)
            .bind(i64::from(limit.schema_version))
            .bind(scope_json)
            .bind(i64::from(limit.max_concurrency))
            .bind(limit.rate.as_ref().map(|rate| i64::from(rate.capacity)))
            .bind(
                limit
                    .rate
                    .as_ref()
                    .map(|rate| i64::from(rate.refill_tokens)),
            )
            .bind(limit.rate.as_ref().map(|rate| rate.refill_interval_ms))
            .bind(existing_created)
            .bind(now.0)
            .execute(&mut *transaction)
            .await
            .map_err(map_insert)?;
            if let Some(rate) = &limit.rate {
                sqlx::query(
                    "INSERT INTO scheduler_buckets (scope_key, tokens, refill_remainder, updated_at_ms)
                     VALUES (?, ?, 0, ?)
                     ON CONFLICT(scope_key) DO UPDATE SET tokens = excluded.tokens,
                        refill_remainder = 0, updated_at_ms = excluded.updated_at_ms",
                )
                .bind(&key)
                .bind(i64::from(rate.capacity))
                .bind(now.0)
                .execute(&mut *transaction)
                .await
                .map_err(map_insert)?;
            } else {
                sqlx::query("DELETE FROM scheduler_buckets WHERE scope_key = ?")
                    .bind(&key)
                    .execute(&mut *transaction)
                    .await
                    .map_err(map_storage)?;
            }
        }
        transaction.commit().await.map_err(map_storage)
    }

    async fn control(
        &self,
        tournament_id: &str,
        next: TournamentWorkerControl,
        now: UnixMillis,
    ) -> Result<(), SchedulerRepositoryError> {
        validate_id(tournament_id)?;
        if now.0 < 0 {
            return Err(SchedulerRepositoryError::Corrupt);
        }
        let mut transaction = self.pool.begin().await.map_err(map_storage)?;
        let existing = sqlx::query(
            "SELECT control, sequence, updated_at_ms FROM tournament_worker_controls
             WHERE tournament_id = ?",
        )
        .bind(tournament_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_storage)?;
        let sequence = if let Some(row) = existing {
            let current = parse_control(text(&row, "control")?.as_str())?;
            if current == TournamentWorkerControl::Cancelled
                || integer(&row, "updated_at_ms")? > now.0
            {
                return Err(SchedulerRepositoryError::Conflict);
            }
            integer(&row, "sequence")?
                .checked_add(1)
                .ok_or(SchedulerRepositoryError::Corrupt)?
        } else {
            0
        };
        sqlx::query(
            "INSERT INTO tournament_worker_controls (
                tournament_id, schema_version, control, sequence, updated_at_ms
             ) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(tournament_id) DO UPDATE SET control = excluded.control,
                sequence = excluded.sequence, updated_at_ms = excluded.updated_at_ms",
        )
        .bind(tournament_id)
        .bind(i64::from(SCHEDULER_SCHEMA_VERSION))
        .bind(control_str(next))
        .bind(sequence)
        .bind(now.0)
        .execute(&mut *transaction)
        .await
        .map_err(map_insert)?;
        if next == TournamentWorkerControl::Cancelled {
            sqlx::query(
                "UPDATE execution_reservations SET status = 'cancel_requested', updated_at_ms = ?
                 WHERE tournament_id = ? AND status = 'active'",
            )
            .bind(now.0)
            .bind(tournament_id)
            .execute(&mut *transaction)
            .await
            .map_err(map_storage)?;
        }
        transaction.commit().await.map_err(map_storage)
    }

    async fn acquire_reservation(
        &self,
        request: ReservationRequest,
    ) -> Result<ReservationResult, SchedulerRepositoryError> {
        request
            .validate()
            .map_err(|_| SchedulerRepositoryError::Corrupt)?;
        let expires_at = request
            .now
            .0
            .checked_add(request.duration_ms)
            .ok_or(SchedulerRepositoryError::Corrupt)?;
        let mut transaction = self.pool.begin().await.map_err(map_storage)?;
        expire_reservations(&mut transaction, request.now).await?;
        if let Some(existing) = load_terminal(&mut transaction, &request.match_id).await? {
            if existing.immutable_inputs_sha256 != request.immutable_inputs_sha256 {
                return Err(SchedulerRepositoryError::Conflict);
            }
            transaction.commit().await.map_err(map_storage)?;
            return Ok(ReservationResult::AlreadyFinished);
        }
        let prior_inputs = sqlx::query_scalar::<_, String>(
            "SELECT immutable_inputs_sha256 FROM execution_reservations
             WHERE match_id = ? ORDER BY acquired_at_ms LIMIT 1",
        )
        .bind(&request.match_id)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(map_storage)?;
        if prior_inputs
            .as_deref()
            .is_some_and(|prior| prior != request.immutable_inputs_sha256)
        {
            return Err(SchedulerRepositoryError::Conflict);
        }
        let control = load_control(&mut transaction, &request.tournament_id).await?;
        match control {
            TournamentWorkerControl::Paused
            | TournamentWorkerControl::Draining
            | TournamentWorkerControl::Cancelled => {
                transaction.commit().await.map_err(map_storage)?;
                return Ok(match control {
                    TournamentWorkerControl::Paused => ReservationResult::Paused,
                    TournamentWorkerControl::Draining => ReservationResult::Draining,
                    TournamentWorkerControl::Cancelled => ReservationResult::Cancelled,
                    TournamentWorkerControl::Running => unreachable!(),
                });
            }
            TournamentWorkerControl::Running => {}
        }
        let mut stored = Vec::new();
        let mut retry_at = None;
        for scope in request.scopes() {
            let limit = load_limit(&mut transaction, &scope.key()).await?;
            let active = active_count(&mut transaction, &scope.key()).await?;
            if active >= i64::from(limit.policy.max_concurrency) {
                transaction.commit().await.map_err(map_storage)?;
                return Ok(ReservationResult::Limited { retry_at: None });
            }
            let mut limit = limit;
            if let (Some(rate), Some(bucket)) = (&limit.policy.rate, limit.bucket) {
                let refilled = refill_bucket(bucket, rate, request.now)
                    .map_err(|_| SchedulerRepositoryError::Corrupt)?;
                if refilled.tokens == 0 {
                    retry_at = max_time(retry_at, token_retry_at(refilled, rate, request.now));
                }
                limit.bucket = Some(refilled);
            }
            stored.push(limit);
        }
        if retry_at.is_some() {
            persist_buckets(&mut transaction, &stored).await?;
            transaction.commit().await.map_err(map_storage)?;
            return Ok(ReservationResult::Limited { retry_at });
        }
        for limit in &mut stored {
            if let Some(bucket) = &mut limit.bucket {
                bucket.tokens = bucket
                    .tokens
                    .checked_sub(1)
                    .ok_or(SchedulerRepositoryError::Corrupt)?;
            }
        }
        persist_buckets(&mut transaction, &stored).await?;
        let request_json =
            serde_json::to_vec(&request).map_err(|_| SchedulerRepositoryError::Corrupt)?;
        insert_reservation(&mut transaction, &request, expires_at, request_json).await?;
        transaction.commit().await.map_err(map_storage)?;
        Ok(ReservationResult::Acquired(Box::new(
            ExecutionReservation {
                request,
                expires_at: UnixMillis(expires_at),
                cancellation_requested: false,
            },
        )))
    }

    async fn renew_reservation(
        &self,
        reservation: ExecutionReservation,
        now: UnixMillis,
        duration_ms: i64,
    ) -> Result<ExecutionReservation, SchedulerRepositoryError> {
        reservation
            .request
            .validate()
            .map_err(|_| SchedulerRepositoryError::Corrupt)?;
        if now.0 < 0 || !(1..=word_arena_application::MAX_RESERVATION_MS).contains(&duration_ms) {
            return Err(SchedulerRepositoryError::Corrupt);
        }
        let expires = now
            .0
            .checked_add(duration_ms)
            .ok_or(SchedulerRepositoryError::Corrupt)?;
        let updated = sqlx::query(
            "UPDATE execution_reservations SET expires_at_ms = ?, updated_at_ms = ?
             WHERE reservation_id = ? AND status = 'active' AND owner = ?
               AND immutable_inputs_sha256 = ? AND expires_at_ms > ? AND ? > expires_at_ms",
        )
        .bind(expires)
        .bind(now.0)
        .bind(&reservation.request.reservation_id)
        .bind(&reservation.request.owner)
        .bind(&reservation.request.immutable_inputs_sha256)
        .bind(now.0)
        .bind(expires)
        .execute(&self.pool)
        .await
        .map_err(map_storage)?;
        if updated.rows_affected() != 1 {
            return Err(SchedulerRepositoryError::Conflict);
        }
        Ok(ExecutionReservation {
            request: reservation.request,
            expires_at: UnixMillis(expires),
            cancellation_requested: false,
        })
    }

    async fn release_reservation(
        &self,
        reservation: ExecutionReservation,
        now: UnixMillis,
    ) -> Result<(), SchedulerRepositoryError> {
        let updated = sqlx::query(
            "UPDATE execution_reservations SET status = 'released', updated_at_ms = ?, finished_at_ms = ?
             WHERE reservation_id = ? AND status IN ('active', 'cancel_requested')
               AND owner = ? AND immutable_inputs_sha256 = ?",
        )
        .bind(now.0).bind(now.0)
        .bind(&reservation.request.reservation_id)
        .bind(&reservation.request.owner)
        .bind(&reservation.request.immutable_inputs_sha256)
        .execute(&self.pool).await.map_err(map_storage)?;
        if updated.rows_affected() == 1 {
            Ok(())
        } else {
            Err(SchedulerRepositoryError::Conflict)
        }
    }

    async fn commit_result(
        &self,
        reservation: ExecutionReservation,
        result: TerminalMatchResult,
        now: UnixMillis,
    ) -> Result<TerminalCommitResult, SchedulerRepositoryError> {
        result
            .validate()
            .map_err(|_| SchedulerRepositoryError::Corrupt)?;
        if result.match_id != reservation.request.match_id
            || result.immutable_inputs_sha256 != reservation.request.immutable_inputs_sha256
        {
            return Err(SchedulerRepositoryError::Conflict);
        }
        let mut transaction = self.pool.begin().await.map_err(map_storage)?;
        if let Some(existing) = load_terminal(&mut transaction, &result.match_id).await? {
            if existing == result {
                transaction.commit().await.map_err(map_storage)?;
                return Ok(TerminalCommitResult::AlreadyApplied(existing));
            }
            return Err(SchedulerRepositoryError::Conflict);
        }
        let updated = sqlx::query(
            "UPDATE execution_reservations SET status = 'completed', updated_at_ms = ?, finished_at_ms = ?
             WHERE reservation_id = ? AND status = 'active' AND owner = ?
               AND match_id = ? AND immutable_inputs_sha256 = ? AND expires_at_ms > ?",
        )
        .bind(now.0).bind(now.0)
        .bind(&reservation.request.reservation_id).bind(&reservation.request.owner)
        .bind(&result.match_id).bind(&result.immutable_inputs_sha256).bind(now.0)
        .execute(&mut *transaction).await.map_err(map_storage)?;
        if updated.rows_affected() != 1 {
            return Err(SchedulerRepositoryError::Conflict);
        }
        let bytes = serde_json::to_vec(&result).map_err(|_| SchedulerRepositoryError::Corrupt)?;
        sqlx::query(
            "INSERT INTO terminal_match_results (
                match_id, schema_version, immutable_inputs_sha256, result_sha256,
                charge_key, telemetry_key, rating_key, result_json, reservation_id, committed_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&result.match_id)
        .bind(i64::from(result.schema_version))
        .bind(&result.immutable_inputs_sha256)
        .bind(&result.result_sha256)
        .bind(&result.charge_key)
        .bind(&result.telemetry_key)
        .bind(&result.rating_key)
        .bind(bytes)
        .bind(&reservation.request.reservation_id)
        .bind(now.0)
        .execute(&mut *transaction)
        .await
        .map_err(map_insert)?;
        transaction.commit().await.map_err(map_storage)?;
        Ok(TerminalCommitResult::Applied(result))
    }

    async fn recovery(
        &self,
        now: UnixMillis,
    ) -> Result<RecoverySnapshot, SchedulerRepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(map_storage)?;
        let expired_match_ids = expire_reservations(&mut transaction, now).await?;
        let controls = sqlx::query(
            "SELECT tournament_id, control FROM tournament_worker_controls ORDER BY tournament_id",
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(map_storage)?
        .into_iter()
        .map(|row| {
            Ok((
                text(&row, "tournament_id")?,
                parse_control(text(&row, "control")?.as_str())?,
            ))
        })
        .collect::<Result<_, SchedulerRepositoryError>>()?;
        let active = sqlx::query(
            "SELECT * FROM execution_reservations
             WHERE status IN ('active', 'cancel_requested') AND expires_at_ms > ?
             ORDER BY acquired_at_ms, reservation_id",
        )
        .bind(now.0)
        .fetch_all(&mut *transaction)
        .await
        .map_err(map_storage)?
        .into_iter()
        .map(|row| reservation(&row))
        .collect::<Result<_, _>>()?;
        transaction.commit().await.map_err(map_storage)?;
        Ok(RecoverySnapshot {
            controls,
            active,
            expired_match_ids,
        })
    }
}

impl SchedulerRepository for SqliteSchedulerRepository {
    fn configure(
        &self,
        limits: Vec<SchedulingLimit>,
        now: UnixMillis,
    ) -> word_arena_application::BoxFuture<'_, Result<(), SchedulerRepositoryError>> {
        Box::pin(self.configure_limits(limits, now))
    }
    fn set_control<'a>(
        &'a self,
        id: &'a str,
        control: TournamentWorkerControl,
        now: UnixMillis,
    ) -> word_arena_application::BoxFuture<'a, Result<(), SchedulerRepositoryError>> {
        Box::pin(self.control(id, control, now))
    }
    fn acquire(
        &self,
        request: ReservationRequest,
    ) -> word_arena_application::BoxFuture<'_, Result<ReservationResult, SchedulerRepositoryError>>
    {
        Box::pin(self.acquire_reservation(request))
    }
    fn renew(
        &self,
        reservation: ExecutionReservation,
        now: UnixMillis,
        duration_ms: i64,
    ) -> word_arena_application::BoxFuture<'_, Result<ExecutionReservation, SchedulerRepositoryError>>
    {
        Box::pin(self.renew_reservation(reservation, now, duration_ms))
    }
    fn release(
        &self,
        reservation: ExecutionReservation,
        now: UnixMillis,
    ) -> word_arena_application::BoxFuture<'_, Result<(), SchedulerRepositoryError>> {
        Box::pin(self.release_reservation(reservation, now))
    }
    fn commit_terminal(
        &self,
        reservation: ExecutionReservation,
        result: TerminalMatchResult,
        now: UnixMillis,
    ) -> word_arena_application::BoxFuture<'_, Result<TerminalCommitResult, SchedulerRepositoryError>>
    {
        Box::pin(self.commit_result(reservation, result, now))
    }
    fn reconstruct(
        &self,
        now: UnixMillis,
    ) -> word_arena_application::BoxFuture<'_, Result<RecoverySnapshot, SchedulerRepositoryError>>
    {
        Box::pin(self.recovery(now))
    }
}

async fn load_limit(
    transaction: &mut Transaction<'_, Sqlite>,
    key: &str,
) -> Result<StoredLimit, SchedulerRepositoryError> {
    let row = sqlx::query(
        "SELECT limits.*, buckets.tokens, buckets.refill_remainder, buckets.updated_at_ms AS bucket_updated_at
         FROM scheduler_limits AS limits LEFT JOIN scheduler_buckets AS buckets USING (scope_key)
         WHERE limits.scope_key = ?",
    ).bind(key).fetch_optional(&mut **transaction).await.map_err(map_storage)?
        .ok_or(SchedulerRepositoryError::Corrupt)?;
    let scope: SchedulerScope = serde_json::from_slice(&bytes(&row, "scope_json")?)
        .map_err(|_| SchedulerRepositoryError::Corrupt)?;
    if scope.key() != key || integer(&row, "schema_version")? != i64::from(SCHEDULER_SCHEMA_VERSION)
    {
        return Err(SchedulerRepositoryError::Corrupt);
    }
    let rate_capacity = optional_integer(&row, "rate_capacity")?;
    let rate = rate_capacity
        .map(|capacity| {
            Ok(RatePolicy {
                capacity: u32::try_from(capacity).map_err(|_| SchedulerRepositoryError::Corrupt)?,
                refill_tokens: u32::try_from(
                    optional_integer(&row, "refill_tokens")?
                        .ok_or(SchedulerRepositoryError::Corrupt)?,
                )
                .map_err(|_| SchedulerRepositoryError::Corrupt)?,
                refill_interval_ms: optional_integer(&row, "refill_interval_ms")?
                    .ok_or(SchedulerRepositoryError::Corrupt)?,
            })
        })
        .transpose()?;
    let policy = SchedulingLimit {
        schema_version: SCHEDULER_SCHEMA_VERSION,
        scope,
        max_concurrency: u32::try_from(integer(&row, "max_concurrency")?)
            .map_err(|_| SchedulerRepositoryError::Corrupt)?,
        rate,
    };
    policy
        .validate()
        .map_err(|_| SchedulerRepositoryError::Corrupt)?;
    let bucket = if policy.rate.is_some() {
        Some(TokenBucketState {
            tokens: u32::try_from(
                optional_integer(&row, "tokens")?.ok_or(SchedulerRepositoryError::Corrupt)?,
            )
            .map_err(|_| SchedulerRepositoryError::Corrupt)?,
            remainder: u64::try_from(
                optional_integer(&row, "refill_remainder")?
                    .ok_or(SchedulerRepositoryError::Corrupt)?,
            )
            .map_err(|_| SchedulerRepositoryError::Corrupt)?,
            updated_at: UnixMillis(
                optional_integer(&row, "bucket_updated_at")?
                    .ok_or(SchedulerRepositoryError::Corrupt)?,
            ),
        })
    } else {
        None
    };
    Ok(StoredLimit { policy, bucket })
}

async fn persist_buckets(
    transaction: &mut Transaction<'_, Sqlite>,
    limits: &[StoredLimit],
) -> Result<(), SchedulerRepositoryError> {
    for limit in limits {
        if let Some(bucket) = limit.bucket {
            sqlx::query("UPDATE scheduler_buckets SET tokens = ?, refill_remainder = ?, updated_at_ms = ? WHERE scope_key = ?")
            .bind(i64::from(bucket.tokens)).bind(i64::try_from(bucket.remainder).map_err(|_| SchedulerRepositoryError::Corrupt)?)
            .bind(bucket.updated_at.0).bind(limit.policy.scope.key()).execute(&mut **transaction).await.map_err(map_storage)?;
        }
    }
    Ok(())
}

async fn active_count(
    transaction: &mut Transaction<'_, Sqlite>,
    key: &str,
) -> Result<i64, SchedulerRepositoryError> {
    sqlx::query_scalar("SELECT COUNT(*) FROM execution_reservation_scopes AS scopes JOIN execution_reservations AS reservations USING (reservation_id) WHERE scopes.scope_key = ? AND reservations.status IN ('active', 'cancel_requested')")
        .bind(key).fetch_one(&mut **transaction).await.map_err(map_storage)
}

async fn insert_reservation(
    transaction: &mut Transaction<'_, Sqlite>,
    request: &ReservationRequest,
    expires: i64,
    json: Vec<u8>,
) -> Result<(), SchedulerRepositoryError> {
    sqlx::query("INSERT INTO execution_reservations (reservation_id, schema_version, job_id, tournament_id, match_id, run_id, harness_id, provider_id, immutable_inputs_sha256, owner, request_json, status, acquired_at_ms, expires_at_ms, updated_at_ms, finished_at_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'active', ?, ?, ?, NULL)")
        .bind(&request.reservation_id).bind(i64::from(request.schema_version)).bind(&request.job_id).bind(&request.tournament_id).bind(&request.match_id).bind(&request.run_id).bind(&request.harness_id).bind(&request.provider_id).bind(&request.immutable_inputs_sha256).bind(&request.owner).bind(json).bind(request.now.0).bind(expires).bind(request.now.0)
        .execute(&mut **transaction).await.map_err(map_insert)?;
    for scope in request.scopes() {
        sqlx::query(
            "INSERT INTO execution_reservation_scopes (reservation_id, scope_key) VALUES (?, ?)",
        )
        .bind(&request.reservation_id)
        .bind(scope.key())
        .execute(&mut **transaction)
        .await
        .map_err(map_insert)?;
    }
    Ok(())
}

async fn expire_reservations(
    transaction: &mut Transaction<'_, Sqlite>,
    now: UnixMillis,
) -> Result<Vec<String>, SchedulerRepositoryError> {
    let rows = sqlx::query("UPDATE execution_reservations SET status = 'expired', updated_at_ms = ?, finished_at_ms = ? WHERE status IN ('active', 'cancel_requested') AND expires_at_ms <= ? RETURNING match_id")
        .bind(now.0).bind(now.0).bind(now.0).fetch_all(&mut **transaction).await.map_err(map_storage)?;
    rows.iter().map(|row| text(row, "match_id")).collect()
}

async fn load_control(
    transaction: &mut Transaction<'_, Sqlite>,
    tournament_id: &str,
) -> Result<TournamentWorkerControl, SchedulerRepositoryError> {
    let value = sqlx::query_scalar::<_, String>(
        "SELECT control FROM tournament_worker_controls WHERE tournament_id = ?",
    )
    .bind(tournament_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage)?;
    value
        .as_deref()
        .map(parse_control)
        .transpose()
        .map(|value| value.unwrap_or(TournamentWorkerControl::Running))
}

async fn load_terminal(
    transaction: &mut Transaction<'_, Sqlite>,
    match_id: &str,
) -> Result<Option<TerminalMatchResult>, SchedulerRepositoryError> {
    let row = sqlx::query("SELECT * FROM terminal_match_results WHERE match_id = ?")
        .bind(match_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(map_storage)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let result: TerminalMatchResult = serde_json::from_slice(&bytes(&row, "result_json")?)
        .map_err(|_| SchedulerRepositoryError::Corrupt)?;
    result
        .validate()
        .map_err(|_| SchedulerRepositoryError::Corrupt)?;
    if text(&row, "match_id")? != result.match_id
        || integer(&row, "schema_version")? != i64::from(result.schema_version)
        || text(&row, "immutable_inputs_sha256")? != result.immutable_inputs_sha256
        || text(&row, "result_sha256")? != result.result_sha256
        || text(&row, "charge_key")? != result.charge_key
        || text(&row, "telemetry_key")? != result.telemetry_key
        || text(&row, "rating_key")? != result.rating_key
    {
        return Err(SchedulerRepositoryError::Corrupt);
    }
    Ok(Some(result))
}

fn reservation(row: &SqliteRow) -> Result<ExecutionReservation, SchedulerRepositoryError> {
    let request: ReservationRequest = serde_json::from_slice(&bytes(row, "request_json")?)
        .map_err(|_| SchedulerRepositoryError::Corrupt)?;
    request
        .validate()
        .map_err(|_| SchedulerRepositoryError::Corrupt)?;
    if text(row, "reservation_id")? != request.reservation_id
        || integer(row, "schema_version")? != i64::from(request.schema_version)
        || text(row, "job_id")? != request.job_id
        || text(row, "tournament_id")? != request.tournament_id
        || text(row, "match_id")? != request.match_id
        || text(row, "run_id")? != request.run_id
        || text(row, "harness_id")? != request.harness_id
        || text(row, "provider_id")? != request.provider_id
        || text(row, "immutable_inputs_sha256")? != request.immutable_inputs_sha256
        || text(row, "owner")? != request.owner
        || integer(row, "acquired_at_ms")? != request.now.0
    {
        return Err(SchedulerRepositoryError::Corrupt);
    }
    Ok(ExecutionReservation {
        request,
        expires_at: UnixMillis(integer(row, "expires_at_ms")?),
        cancellation_requested: text(row, "status")? == "cancel_requested",
    })
}

fn parse_control(value: &str) -> Result<TournamentWorkerControl, SchedulerRepositoryError> {
    match value {
        "running" => Ok(TournamentWorkerControl::Running),
        "paused" => Ok(TournamentWorkerControl::Paused),
        "draining" => Ok(TournamentWorkerControl::Draining),
        "cancelled" => Ok(TournamentWorkerControl::Cancelled),
        _ => Err(SchedulerRepositoryError::Corrupt),
    }
}
const fn control_str(value: TournamentWorkerControl) -> &'static str {
    match value {
        TournamentWorkerControl::Running => "running",
        TournamentWorkerControl::Paused => "paused",
        TournamentWorkerControl::Draining => "draining",
        TournamentWorkerControl::Cancelled => "cancelled",
    }
}
fn max_time(left: Option<UnixMillis>, right: Option<UnixMillis>) -> Option<UnixMillis> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (left, right) => left.or(right),
    }
}
fn bytes(row: &SqliteRow, column: &str) -> Result<Vec<u8>, SchedulerRepositoryError> {
    row.try_get(column)
        .map_err(|_| SchedulerRepositoryError::Corrupt)
}
fn text(row: &SqliteRow, column: &str) -> Result<String, SchedulerRepositoryError> {
    row.try_get(column)
        .map_err(|_| SchedulerRepositoryError::Corrupt)
}
fn integer(row: &SqliteRow, column: &str) -> Result<i64, SchedulerRepositoryError> {
    row.try_get(column)
        .map_err(|_| SchedulerRepositoryError::Corrupt)
}
fn optional_integer(
    row: &SqliteRow,
    column: &str,
) -> Result<Option<i64>, SchedulerRepositoryError> {
    row.try_get(column)
        .map_err(|_| SchedulerRepositoryError::Corrupt)
}
fn validate_id(value: &str) -> Result<(), SchedulerRepositoryError> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        Err(SchedulerRepositoryError::Corrupt)
    } else {
        Ok(())
    }
}
fn map_insert(error: sqlx::Error) -> SchedulerRepositoryError {
    if let sqlx::Error::Database(database) = &error
        && (database.is_unique_violation()
            || database.is_foreign_key_violation()
            || database.is_check_violation())
    {
        SchedulerRepositoryError::Conflict
    } else {
        map_storage(error)
    }
}
fn map_storage(_error: sqlx::Error) -> SchedulerRepositoryError {
    SchedulerRepositoryError::Unavailable
}
