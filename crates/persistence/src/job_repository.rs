use sqlx::{QueryBuilder, Row, Sqlite, Transaction, sqlite::SqliteRow};
use word_arena_application::{
    CancellationResult, ClaimJobs, CompletionResult, EnqueueResult, JOB_MAX_LEASE_MS,
    JobHandlerOutcome, JobLease, JobRecord, JobRepository, JobRepositoryError, JobStatus, NewJob,
    RenewalResult, UnixMillis, retry_backoff_ms,
};

#[derive(Clone, Debug)]
pub struct SqliteJobRepository {
    pool: sqlx::SqlitePool,
}

impl SqliteJobRepository {
    #[must_use]
    pub const fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    async fn enqueue_job(
        &self,
        job: NewJob,
        now: UnixMillis,
    ) -> Result<EnqueueResult, JobRepositoryError> {
        job.validate().map_err(|_| JobRepositoryError::Corrupt)?;
        if now.0 < 0 {
            return Err(JobRepositoryError::Corrupt);
        }
        let payload_sha256 = job
            .payload_sha256()
            .map_err(|_| JobRepositoryError::Corrupt)?;
        let result = sqlx::query(
            "INSERT INTO jobs (
                job_id, schema_version, kind, payload_schema_version, payload_json,
                payload_sha256, priority, available_at_ms, max_attempts, attempt,
                retry_initial_ms, retry_max_ms, deduplication_key, status, owner,
                lease_generation, leased_at_ms, lease_expires_at_ms,
                cancellation_requested_at_ms, last_error_code, created_at_ms,
                updated_at_ms, finished_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, 'queued', NULL, 0,
                       NULL, NULL, NULL, NULL, ?, ?, NULL)",
        )
        .bind(&job.job_id)
        .bind(i64::from(job.schema_version))
        .bind(&job.kind)
        .bind(i64::from(job.payload_schema_version))
        .bind(&job.payload_json)
        .bind(&payload_sha256)
        .bind(job.priority)
        .bind(job.available_at.0)
        .bind(i64::from(job.max_attempts))
        .bind(job.retry_initial_ms)
        .bind(job.retry_max_ms)
        .bind(&job.deduplication_key)
        .bind(now.0)
        .bind(now.0)
        .execute(&self.pool)
        .await;
        match result {
            Ok(_) => Ok(EnqueueResult::Inserted(self.load_job(&job.job_id).await?)),
            Err(error) if is_unique(&error) => {
                let existing = self
                    .load_deduplicated(&job.kind, &job.deduplication_key)
                    .await?;
                if same_enqueue_identity(&existing, &job, &payload_sha256) {
                    Ok(EnqueueResult::Existing(existing))
                } else {
                    Err(JobRepositoryError::Conflict)
                }
            }
            Err(error) => Err(map_storage(error)),
        }
    }

    async fn load_job(&self, job_id: &str) -> Result<JobRecord, JobRepositoryError> {
        validate_id(job_id)?;
        let row = sqlx::query("SELECT * FROM jobs WHERE job_id = ?")
            .bind(job_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_storage)?
            .ok_or(JobRepositoryError::NotFound)?;
        record(&row)
    }

    async fn load_deduplicated(
        &self,
        kind: &str,
        key: &str,
    ) -> Result<JobRecord, JobRepositoryError> {
        let row = sqlx::query("SELECT * FROM jobs WHERE kind = ? AND deduplication_key = ?")
            .bind(kind)
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_storage)?
            .ok_or(JobRepositoryError::Conflict)?;
        record(&row)
    }

    async fn claim_job(&self, request: ClaimJobs) -> Result<Option<JobLease>, JobRepositoryError> {
        request
            .validate()
            .map_err(|_| JobRepositoryError::Corrupt)?;
        let expires_at = request
            .now
            .0
            .checked_add(request.lease_duration_ms)
            .ok_or(JobRepositoryError::Corrupt)?;
        let mut transaction = self.pool.begin().await.map_err(map_storage)?;
        recover_expired(&mut transaction, request.now).await?;

        let mut query = QueryBuilder::<Sqlite>::new(
            "UPDATE jobs SET status = 'leased', attempt = attempt + 1,
             lease_generation = lease_generation + 1, owner = ",
        );
        query
            .push_bind(&request.worker_id)
            .push(", leased_at_ms = ")
            .push_bind(request.now.0)
            .push(", lease_expires_at_ms = ")
            .push_bind(expires_at)
            .push(", updated_at_ms = ")
            .push_bind(request.now.0)
            .push(
                ", last_error_code = NULL WHERE job_id = (
                SELECT job_id FROM jobs
                WHERE cancellation_requested_at_ms IS NULL
                  AND attempt < max_attempts
                  AND ((status = 'queued' AND available_at_ms <= ",
            )
            .push_bind(request.now.0)
            .push(") OR (status = 'leased' AND lease_expires_at_ms <= ")
            .push_bind(request.now.0)
            .push(")) AND kind IN (");
        {
            let mut separated = query.separated(", ");
            for kind in &request.kinds {
                separated.push_bind(kind);
            }
        }
        query.push(
            ") ORDER BY priority DESC, available_at_ms ASC, created_at_ms ASC,
             job_id ASC LIMIT 1) RETURNING *",
        );
        let Some(row) = query
            .build()
            .fetch_optional(&mut *transaction)
            .await
            .map_err(map_storage)?
        else {
            transaction.commit().await.map_err(map_storage)?;
            return Ok(None);
        };
        let job = record(&row)?;
        let generation = u64::try_from(integer(&row, "lease_generation")?)
            .map_err(|_| JobRepositoryError::Corrupt)?;
        sqlx::query(
            "INSERT INTO job_attempts (
                job_id, attempt, lease_generation, worker_id, leased_at_ms,
                lease_expires_at_ms, finished_at_ms, handler_outcome, final_status,
                error_code, next_available_at_ms
             ) VALUES (?, ?, ?, ?, ?, ?, NULL, NULL, NULL, NULL, NULL)",
        )
        .bind(&job.job_id)
        .bind(i64::from(job.attempt))
        .bind(i64::try_from(generation).map_err(|_| JobRepositoryError::Corrupt)?)
        .bind(&request.worker_id)
        .bind(request.now.0)
        .bind(expires_at)
        .execute(&mut *transaction)
        .await
        .map_err(map_insert)?;
        transaction.commit().await.map_err(map_storage)?;
        Ok(Some(JobLease {
            job,
            worker_id: request.worker_id,
            lease_generation: generation,
            leased_at: request.now,
            lease_expires_at: UnixMillis(expires_at),
        }))
    }

    async fn renew_job(
        &self,
        lease: JobLease,
        now: UnixMillis,
        duration_ms: i64,
    ) -> Result<RenewalResult, JobRepositoryError> {
        if now.0 < 0 || !(1..=JOB_MAX_LEASE_MS).contains(&duration_ms) {
            return Err(JobRepositoryError::Corrupt);
        }
        let expires_at = now
            .0
            .checked_add(duration_ms)
            .ok_or(JobRepositoryError::Corrupt)?;
        let mut transaction = self.pool.begin().await.map_err(map_storage)?;
        let updated = sqlx::query(
            "UPDATE jobs SET lease_expires_at_ms = ?, updated_at_ms = ?
             WHERE job_id = ? AND status = 'leased' AND owner = ?
               AND attempt = ? AND lease_generation = ?
               AND lease_expires_at_ms > ? AND ? > lease_expires_at_ms",
        )
        .bind(expires_at)
        .bind(now.0)
        .bind(&lease.job.job_id)
        .bind(&lease.worker_id)
        .bind(i64::from(lease.job.attempt))
        .bind(i64::try_from(lease.lease_generation).map_err(|_| JobRepositoryError::Corrupt)?)
        .bind(now.0)
        .bind(expires_at)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage)?;
        if updated.rows_affected() != 1 {
            return Err(JobRepositoryError::Conflict);
        }
        let attempt_updated = sqlx::query(
            "UPDATE job_attempts SET lease_expires_at_ms = ?
             WHERE job_id = ? AND attempt = ? AND lease_generation = ?
               AND finished_at_ms IS NULL",
        )
        .bind(expires_at)
        .bind(&lease.job.job_id)
        .bind(i64::from(lease.job.attempt))
        .bind(i64::try_from(lease.lease_generation).map_err(|_| JobRepositoryError::Corrupt)?)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage)?;
        if attempt_updated.rows_affected() != 1 {
            return Err(JobRepositoryError::Corrupt);
        }
        let cancelled = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT cancellation_requested_at_ms FROM jobs WHERE job_id = ?",
        )
        .bind(&lease.job.job_id)
        .fetch_one(&mut *transaction)
        .await
        .map_err(map_storage)?;
        transaction.commit().await.map_err(map_storage)?;
        Ok(if cancelled.is_some() {
            RenewalResult::CancellationRequested
        } else {
            RenewalResult::Renewed
        })
    }

    async fn complete_job(
        &self,
        lease: JobLease,
        outcome: JobHandlerOutcome,
        now: UnixMillis,
    ) -> Result<CompletionResult, JobRepositoryError> {
        outcome
            .validate()
            .map_err(|_| JobRepositoryError::Corrupt)?;
        if now.0 < 0 {
            return Err(JobRepositoryError::Corrupt);
        }
        let mut transaction = self.pool.begin().await.map_err(map_storage)?;
        if let Some(result) = existing_completion(&mut transaction, &lease, &outcome).await? {
            transaction.commit().await.map_err(map_storage)?;
            return Ok(CompletionResult::AlreadyApplied(result));
        }
        let row = sqlx::query("SELECT * FROM jobs WHERE job_id = ?")
            .bind(&lease.job.job_id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(map_storage)?
            .ok_or(JobRepositoryError::NotFound)?;
        let current = record(&row)?;
        validate_active_completion(&row, &current, &lease, &outcome, now)?;
        let (handler_outcome, error_code) = outcome_columns(&outcome);
        let (status, available_at, finished_at) = completion_transition(&current, &outcome, now)?;
        let updated = sqlx::query(
            "UPDATE jobs SET status = ?, available_at_ms = ?, owner = NULL,
                leased_at_ms = NULL, lease_expires_at_ms = NULL, last_error_code = ?,
                updated_at_ms = ?, finished_at_ms = ?
             WHERE job_id = ? AND status = 'leased' AND owner = ?
               AND attempt = ? AND lease_generation = ? AND lease_expires_at_ms > ?",
        )
        .bind(status.as_str())
        .bind(available_at)
        .bind(error_code)
        .bind(now.0)
        .bind(finished_at)
        .bind(&current.job_id)
        .bind(&lease.worker_id)
        .bind(i64::from(current.attempt))
        .bind(i64::try_from(lease.lease_generation).map_err(|_| JobRepositoryError::Corrupt)?)
        .bind(now.0)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage)?;
        if updated.rows_affected() != 1 {
            return Err(JobRepositoryError::Conflict);
        }
        let attempt_updated = sqlx::query(
            "UPDATE job_attempts SET finished_at_ms = ?, handler_outcome = ?,
                final_status = ?, error_code = ?, next_available_at_ms = ?
             WHERE job_id = ? AND attempt = ? AND lease_generation = ?
               AND finished_at_ms IS NULL",
        )
        .bind(now.0)
        .bind(handler_outcome)
        .bind(status.as_str())
        .bind(error_code)
        .bind((status == JobStatus::Queued).then_some(available_at))
        .bind(&current.job_id)
        .bind(i64::from(current.attempt))
        .bind(i64::try_from(lease.lease_generation).map_err(|_| JobRepositoryError::Corrupt)?)
        .execute(&mut *transaction)
        .await
        .map_err(map_storage)?;
        if attempt_updated.rows_affected() != 1 {
            return Err(JobRepositoryError::Corrupt);
        }
        let result = load_in_transaction(&mut transaction, &current.job_id).await?;
        transaction.commit().await.map_err(map_storage)?;
        Ok(CompletionResult::Applied(result))
    }

    async fn cancel_job(
        &self,
        job_id: &str,
        now: UnixMillis,
    ) -> Result<CancellationResult, JobRepositoryError> {
        validate_id(job_id)?;
        if now.0 < 0 {
            return Err(JobRepositoryError::Corrupt);
        }
        let row = sqlx::query("SELECT status, created_at_ms FROM jobs WHERE job_id = ?")
            .bind(job_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_storage)?
            .ok_or(JobRepositoryError::NotFound)?;
        let status = status(text(&row, "status")?.as_str())?;
        if status.is_terminal() {
            return Ok(CancellationResult::AlreadyTerminal);
        }
        if status == JobStatus::Queued {
            let updated = sqlx::query(
                "UPDATE jobs SET status = 'cancelled', cancellation_requested_at_ms = ?,
                    updated_at_ms = ?, finished_at_ms = ?
                 WHERE job_id = ? AND status = 'queued'",
            )
            .bind(now.0)
            .bind(now.0)
            .bind(now.0)
            .bind(job_id)
            .execute(&self.pool)
            .await
            .map_err(map_storage)?;
            return if updated.rows_affected() == 1 {
                Ok(CancellationResult::Cancelled)
            } else {
                Err(JobRepositoryError::Conflict)
            };
        }
        let updated = sqlx::query(
            "UPDATE jobs SET cancellation_requested_at_ms = COALESCE(
                    cancellation_requested_at_ms, ?), updated_at_ms = ?
             WHERE job_id = ? AND status = 'leased'",
        )
        .bind(now.0)
        .bind(now.0)
        .bind(job_id)
        .execute(&self.pool)
        .await
        .map_err(map_storage)?;
        if updated.rows_affected() == 1 {
            Ok(CancellationResult::Requested)
        } else {
            Err(JobRepositoryError::Conflict)
        }
    }
}

impl JobRepository for SqliteJobRepository {
    fn enqueue(
        &self,
        job: NewJob,
        now: UnixMillis,
    ) -> word_arena_application::BoxFuture<'_, Result<EnqueueResult, JobRepositoryError>> {
        Box::pin(self.enqueue_job(job, now))
    }

    fn load<'a>(
        &'a self,
        job_id: &'a str,
    ) -> word_arena_application::BoxFuture<'a, Result<JobRecord, JobRepositoryError>> {
        Box::pin(self.load_job(job_id))
    }

    fn claim(
        &self,
        request: ClaimJobs,
    ) -> word_arena_application::BoxFuture<'_, Result<Option<JobLease>, JobRepositoryError>> {
        Box::pin(self.claim_job(request))
    }

    fn renew(
        &self,
        lease: JobLease,
        now: UnixMillis,
        lease_duration_ms: i64,
    ) -> word_arena_application::BoxFuture<'_, Result<RenewalResult, JobRepositoryError>> {
        Box::pin(self.renew_job(lease, now, lease_duration_ms))
    }

    fn complete(
        &self,
        lease: JobLease,
        outcome: JobHandlerOutcome,
        now: UnixMillis,
    ) -> word_arena_application::BoxFuture<'_, Result<CompletionResult, JobRepositoryError>> {
        Box::pin(self.complete_job(lease, outcome, now))
    }

    fn cancel<'a>(
        &'a self,
        job_id: &'a str,
        now: UnixMillis,
    ) -> word_arena_application::BoxFuture<'a, Result<CancellationResult, JobRepositoryError>> {
        Box::pin(self.cancel_job(job_id, now))
    }
}

async fn recover_expired(
    transaction: &mut Transaction<'_, Sqlite>,
    now: UnixMillis,
) -> Result<(), JobRepositoryError> {
    sqlx::query(
        "UPDATE job_attempts SET finished_at_ms = ?, handler_outcome = 'abandoned',
            final_status = CASE
                WHEN (SELECT cancellation_requested_at_ms FROM jobs WHERE jobs.job_id = job_attempts.job_id) IS NOT NULL THEN 'cancelled'
                WHEN attempt >= (SELECT max_attempts FROM jobs WHERE jobs.job_id = job_attempts.job_id) THEN 'exhausted'
                ELSE 'queued' END,
            next_available_at_ms = CASE
                WHEN attempt < (SELECT max_attempts FROM jobs WHERE jobs.job_id = job_attempts.job_id) THEN ?
                ELSE NULL END
         WHERE finished_at_ms IS NULL AND lease_expires_at_ms <= ?",
    )
    .bind(now.0)
    .bind(now.0)
    .bind(now.0)
    .execute(&mut **transaction)
    .await
    .map_err(map_storage)?;
    sqlx::query(
        "UPDATE jobs SET status = CASE
                WHEN cancellation_requested_at_ms IS NOT NULL THEN 'cancelled'
                ELSE 'exhausted' END,
            owner = NULL, leased_at_ms = NULL, lease_expires_at_ms = NULL,
            updated_at_ms = ?, finished_at_ms = ?
         WHERE status = 'leased' AND lease_expires_at_ms <= ?
           AND (cancellation_requested_at_ms IS NOT NULL OR attempt >= max_attempts)",
    )
    .bind(now.0)
    .bind(now.0)
    .bind(now.0)
    .execute(&mut **transaction)
    .await
    .map_err(map_storage)?;
    Ok(())
}

async fn existing_completion(
    transaction: &mut Transaction<'_, Sqlite>,
    lease: &JobLease,
    outcome: &JobHandlerOutcome,
) -> Result<Option<JobRecord>, JobRepositoryError> {
    let row = sqlx::query(
        "SELECT handler_outcome, error_code FROM job_attempts
         WHERE job_id = ? AND attempt = ? AND lease_generation = ?",
    )
    .bind(&lease.job.job_id)
    .bind(i64::from(lease.job.attempt))
    .bind(i64::try_from(lease.lease_generation).map_err(|_| JobRepositoryError::Corrupt)?)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(map_storage)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let stored: Option<String> = row
        .try_get("handler_outcome")
        .map_err(|_| JobRepositoryError::Corrupt)?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    let stored_error: Option<String> = row
        .try_get("error_code")
        .map_err(|_| JobRepositoryError::Corrupt)?;
    let (requested, requested_error) = outcome_columns(outcome);
    if stored != requested || stored_error.as_deref() != requested_error {
        return Err(JobRepositoryError::Conflict);
    }
    Ok(Some(
        load_in_transaction(transaction, &lease.job.job_id).await?,
    ))
}

async fn load_in_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    job_id: &str,
) -> Result<JobRecord, JobRepositoryError> {
    let row = sqlx::query("SELECT * FROM jobs WHERE job_id = ?")
        .bind(job_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(map_storage)?;
    record(&row)
}

fn record(row: &SqliteRow) -> Result<JobRecord, JobRepositoryError> {
    let job = JobRecord {
        schema_version: u32::try_from(integer(row, "schema_version")?)
            .map_err(|_| JobRepositoryError::Corrupt)?,
        job_id: text(row, "job_id")?,
        kind: text(row, "kind")?,
        payload_schema_version: u32::try_from(integer(row, "payload_schema_version")?)
            .map_err(|_| JobRepositoryError::Corrupt)?,
        payload_json: row
            .try_get("payload_json")
            .map_err(|_| JobRepositoryError::Corrupt)?,
        payload_sha256: text(row, "payload_sha256")?,
        priority: row
            .try_get("priority")
            .map_err(|_| JobRepositoryError::Corrupt)?,
        available_at: UnixMillis(integer(row, "available_at_ms")?),
        max_attempts: u32::try_from(integer(row, "max_attempts")?)
            .map_err(|_| JobRepositoryError::Corrupt)?,
        attempt: u32::try_from(integer(row, "attempt")?)
            .map_err(|_| JobRepositoryError::Corrupt)?,
        retry_initial_ms: integer(row, "retry_initial_ms")?,
        retry_max_ms: integer(row, "retry_max_ms")?,
        deduplication_key: text(row, "deduplication_key")?,
        status: status(text(row, "status")?.as_str())?,
        owner: row
            .try_get("owner")
            .map_err(|_| JobRepositoryError::Corrupt)?,
        lease_generation: u64::try_from(integer(row, "lease_generation")?)
            .map_err(|_| JobRepositoryError::Corrupt)?,
        leased_at: optional_integer(row, "leased_at_ms")?.map(UnixMillis),
        lease_expires_at: optional_integer(row, "lease_expires_at_ms")?.map(UnixMillis),
        cancellation_requested_at: optional_integer(row, "cancellation_requested_at_ms")?
            .map(UnixMillis),
        created_at: UnixMillis(integer(row, "created_at_ms")?),
        updated_at: UnixMillis(integer(row, "updated_at_ms")?),
        finished_at: optional_integer(row, "finished_at_ms")?.map(UnixMillis),
    };
    let input = NewJob {
        schema_version: job.schema_version,
        job_id: job.job_id.clone(),
        kind: job.kind.clone(),
        payload_schema_version: job.payload_schema_version,
        payload_json: job.payload_json.clone(),
        priority: job.priority,
        available_at: job.available_at,
        max_attempts: job.max_attempts,
        retry_initial_ms: job.retry_initial_ms,
        retry_max_ms: job.retry_max_ms,
        deduplication_key: job.deduplication_key.clone(),
    };
    if input.validate().is_err()
        || input.payload_sha256().ok().as_deref() != Some(job.payload_sha256.as_str())
        || job.attempt > job.max_attempts
        || job.updated_at < job.created_at
        || job.status.is_terminal() != job.finished_at.is_some()
        || (job.status == JobStatus::Leased)
            != (job.owner.is_some() && job.leased_at.is_some() && job.lease_expires_at.is_some())
        || job.status != JobStatus::Leased
            && (job.owner.is_some() || job.leased_at.is_some() || job.lease_expires_at.is_some())
        || job
            .leased_at
            .zip(job.lease_expires_at)
            .is_some_and(|(leased, expires)| expires <= leased)
    {
        return Err(JobRepositoryError::Corrupt);
    }
    Ok(job)
}

fn same_enqueue_identity(existing: &JobRecord, new: &NewJob, payload_sha256: &str) -> bool {
    existing.schema_version == new.schema_version
        && existing.kind == new.kind
        && existing.payload_schema_version == new.payload_schema_version
        && existing.payload_json == new.payload_json
        && existing.payload_sha256 == payload_sha256
        && existing.priority == new.priority
        && existing.available_at == new.available_at
        && existing.max_attempts == new.max_attempts
        && existing.retry_initial_ms == new.retry_initial_ms
        && existing.retry_max_ms == new.retry_max_ms
        && existing.deduplication_key == new.deduplication_key
}

fn validate_active_completion(
    row: &SqliteRow,
    current: &JobRecord,
    lease: &JobLease,
    outcome: &JobHandlerOutcome,
    now: UnixMillis,
) -> Result<(), JobRepositoryError> {
    let owner: Option<String> = row
        .try_get("owner")
        .map_err(|_| JobRepositoryError::Corrupt)?;
    let generation = u64::try_from(integer(row, "lease_generation")?)
        .map_err(|_| JobRepositoryError::Corrupt)?;
    let expiry =
        optional_integer(row, "lease_expires_at_ms")?.ok_or(JobRepositoryError::Corrupt)?;
    if current.status != JobStatus::Leased
        || current.attempt != lease.job.attempt
        || owner.as_deref() != Some(lease.worker_id.as_str())
        || generation != lease.lease_generation
        || expiry <= now.0
        || current.cancellation_requested_at.is_some() && *outcome != JobHandlerOutcome::Cancelled
    {
        Err(JobRepositoryError::Conflict)
    } else {
        Ok(())
    }
}

fn completion_transition(
    current: &JobRecord,
    outcome: &JobHandlerOutcome,
    now: UnixMillis,
) -> Result<(JobStatus, i64, Option<i64>), JobRepositoryError> {
    match outcome {
        JobHandlerOutcome::Succeeded => {
            Ok((JobStatus::Succeeded, current.available_at.0, Some(now.0)))
        }
        JobHandlerOutcome::Permanent { .. } => Ok((
            JobStatus::PermanentFailure,
            current.available_at.0,
            Some(now.0),
        )),
        JobHandlerOutcome::Cancelled => {
            Ok((JobStatus::Cancelled, current.available_at.0, Some(now.0)))
        }
        JobHandlerOutcome::Retryable { .. } if current.attempt >= current.max_attempts => {
            Ok((JobStatus::Exhausted, current.available_at.0, Some(now.0)))
        }
        JobHandlerOutcome::Retryable { .. } => {
            let backoff = retry_backoff_ms(
                current.retry_initial_ms,
                current.retry_max_ms,
                current.attempt,
            );
            let next = now
                .0
                .checked_add(backoff)
                .ok_or(JobRepositoryError::Corrupt)?;
            Ok((JobStatus::Queued, next, None))
        }
    }
}

fn outcome_columns(outcome: &JobHandlerOutcome) -> (&'static str, Option<&str>) {
    match outcome {
        JobHandlerOutcome::Succeeded => ("succeeded", None),
        JobHandlerOutcome::Retryable { error_code } => ("retryable", Some(error_code)),
        JobHandlerOutcome::Permanent { error_code } => ("permanent", Some(error_code)),
        JobHandlerOutcome::Cancelled => ("cancelled", None),
    }
}

fn status(value: &str) -> Result<JobStatus, JobRepositoryError> {
    match value {
        "queued" => Ok(JobStatus::Queued),
        "leased" => Ok(JobStatus::Leased),
        "succeeded" => Ok(JobStatus::Succeeded),
        "permanent_failure" => Ok(JobStatus::PermanentFailure),
        "exhausted" => Ok(JobStatus::Exhausted),
        "cancelled" => Ok(JobStatus::Cancelled),
        _ => Err(JobRepositoryError::Corrupt),
    }
}

fn text(row: &SqliteRow, column: &str) -> Result<String, JobRepositoryError> {
    row.try_get(column).map_err(|_| JobRepositoryError::Corrupt)
}

fn integer(row: &SqliteRow, column: &str) -> Result<i64, JobRepositoryError> {
    row.try_get(column).map_err(|_| JobRepositoryError::Corrupt)
}

fn optional_integer(row: &SqliteRow, column: &str) -> Result<Option<i64>, JobRepositoryError> {
    row.try_get(column).map_err(|_| JobRepositoryError::Corrupt)
}

fn validate_id(value: &str) -> Result<(), JobRepositoryError> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        Err(JobRepositoryError::Corrupt)
    } else {
        Ok(())
    }
}

fn is_unique(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(database) if database.is_unique_violation())
}

fn map_insert(error: sqlx::Error) -> JobRepositoryError {
    if let sqlx::Error::Database(database) = &error
        && (database.is_unique_violation()
            || database.is_foreign_key_violation()
            || database.is_check_violation())
    {
        return JobRepositoryError::Conflict;
    }
    map_storage(error)
}

fn map_storage(_error: sqlx::Error) -> JobRepositoryError {
    JobRepositoryError::Unavailable
}
