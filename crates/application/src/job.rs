use std::{collections::BTreeSet, fmt::Debug, fmt::Write, sync::Arc};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{ApplicationClock, BoxFuture, UnixMillis};

pub const JOB_SCHEMA_VERSION: u32 = 1;
pub const JOB_PAYLOAD_MAX_BYTES: usize = 1_048_576;
pub const JOB_MAX_ATTEMPTS: u32 = 100;
pub const JOB_MAX_LEASE_MS: i64 = 86_400_000;
pub const JOB_MAX_BACKOFF_MS: i64 = 604_800_000;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NewJob {
    pub schema_version: u32,
    pub job_id: String,
    pub kind: String,
    pub payload_schema_version: u32,
    pub payload_json: Vec<u8>,
    pub priority: i32,
    pub available_at: UnixMillis,
    pub max_attempts: u32,
    pub retry_initial_ms: i64,
    pub retry_max_ms: i64,
    pub deduplication_key: String,
}

impl NewJob {
    /// Validates the strict durable job and canonical JSON payload contract.
    ///
    /// # Errors
    ///
    /// Returns [`JobError::InvalidJob`] for malformed or unbounded input.
    pub fn validate(&self) -> Result<(), JobError> {
        if self.schema_version != JOB_SCHEMA_VERSION
            || !valid_id(&self.job_id, 256)
            || !valid_kind(&self.kind)
            || self.payload_schema_version == 0
            || self.payload_json.is_empty()
            || self.payload_json.len() > JOB_PAYLOAD_MAX_BYTES
            || self.available_at.0 < 0
            || !(1..=JOB_MAX_ATTEMPTS).contains(&self.max_attempts)
            || self.retry_initial_ms <= 0
            || self.retry_max_ms < self.retry_initial_ms
            || self.retry_max_ms > JOB_MAX_BACKOFF_MS
            || !valid_id(&self.deduplication_key, 256)
        {
            return Err(JobError::InvalidJob);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&self.payload_json).map_err(|_| JobError::InvalidJob)?;
        if serde_json::to_vec(&value).map_err(|_| JobError::InvalidJob)? != self.payload_json {
            return Err(JobError::InvalidJob);
        }
        Ok(())
    }

    /// Returns the SHA-256 identity of the canonical payload bytes.
    ///
    /// # Errors
    ///
    /// Returns [`JobError::InvalidJob`] when the job contract is invalid.
    pub fn payload_sha256(&self) -> Result<String, JobError> {
        self.validate()?;
        Ok(hex_digest(&self.payload_json))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Leased,
    Succeeded,
    PermanentFailure,
    Exhausted,
    Cancelled,
}

impl JobStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Leased => "leased",
            Self::Succeeded => "succeeded",
            Self::PermanentFailure => "permanent_failure",
            Self::Exhausted => "exhausted",
            Self::Cancelled => "cancelled",
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::PermanentFailure | Self::Exhausted | Self::Cancelled
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JobRecord {
    pub schema_version: u32,
    pub job_id: String,
    pub kind: String,
    pub payload_schema_version: u32,
    pub payload_json: Vec<u8>,
    pub payload_sha256: String,
    pub priority: i32,
    pub available_at: UnixMillis,
    pub max_attempts: u32,
    pub attempt: u32,
    pub retry_initial_ms: i64,
    pub retry_max_ms: i64,
    pub deduplication_key: String,
    pub status: JobStatus,
    pub owner: Option<String>,
    pub lease_generation: u64,
    pub leased_at: Option<UnixMillis>,
    pub lease_expires_at: Option<UnixMillis>,
    pub cancellation_requested_at: Option<UnixMillis>,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
    pub finished_at: Option<UnixMillis>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JobLease {
    pub job: JobRecord,
    pub worker_id: String,
    pub lease_generation: u64,
    pub leased_at: UnixMillis,
    pub lease_expires_at: UnixMillis,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimJobs {
    pub worker_id: String,
    pub kinds: BTreeSet<String>,
    pub now: UnixMillis,
    pub lease_duration_ms: i64,
}

impl ClaimJobs {
    /// Validates worker, kind, clock, and lease bounds.
    ///
    /// # Errors
    ///
    /// Returns [`JobError::InvalidClaim`] for unsafe claim input.
    pub fn validate(&self) -> Result<(), JobError> {
        if !valid_id(&self.worker_id, 128)
            || self.kinds.is_empty()
            || self.kinds.iter().any(|kind| !valid_kind(kind))
            || self.now.0 < 0
            || !(1..=JOB_MAX_LEASE_MS).contains(&self.lease_duration_ms)
        {
            Err(JobError::InvalidClaim)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnqueueResult {
    Inserted(JobRecord),
    Existing(JobRecord),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum JobHandlerOutcome {
    Succeeded,
    Retryable { error_code: String },
    Permanent { error_code: String },
    Cancelled,
}

impl JobHandlerOutcome {
    /// Validates stable public error codes on non-success outcomes.
    ///
    /// # Errors
    ///
    /// Returns [`JobError::InvalidOutcome`] for an unsafe error code.
    pub fn validate(&self) -> Result<(), JobError> {
        match self {
            Self::Retryable { error_code } | Self::Permanent { error_code } => {
                if !valid_error_code(error_code) {
                    return Err(JobError::InvalidOutcome);
                }
            }
            Self::Succeeded | Self::Cancelled => {}
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompletionResult {
    Applied(JobRecord),
    AlreadyApplied(JobRecord),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenewalResult {
    Renewed,
    CancellationRequested,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancellationResult {
    Cancelled,
    Requested,
    AlreadyTerminal,
}

pub trait JobRepository: Debug + Send + Sync {
    fn enqueue(
        &self,
        job: NewJob,
        now: UnixMillis,
    ) -> BoxFuture<'_, Result<EnqueueResult, JobRepositoryError>>;
    fn load<'a>(&'a self, job_id: &'a str) -> BoxFuture<'a, Result<JobRecord, JobRepositoryError>>;
    fn claim(
        &self,
        request: ClaimJobs,
    ) -> BoxFuture<'_, Result<Option<JobLease>, JobRepositoryError>>;
    fn renew(
        &self,
        lease: JobLease,
        now: UnixMillis,
        lease_duration_ms: i64,
    ) -> BoxFuture<'_, Result<RenewalResult, JobRepositoryError>>;
    fn complete(
        &self,
        lease: JobLease,
        outcome: JobHandlerOutcome,
        now: UnixMillis,
    ) -> BoxFuture<'_, Result<CompletionResult, JobRepositoryError>>;
    fn cancel<'a>(
        &'a self,
        job_id: &'a str,
        now: UnixMillis,
    ) -> BoxFuture<'a, Result<CancellationResult, JobRepositoryError>>;
}

pub trait JobHandler: Debug + Send + Sync {
    fn kind(&self) -> &str;
    fn handle<'a>(&'a self, lease: &'a JobLease) -> BoxFuture<'a, JobHandlerOutcome>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerStep {
    Idle,
    Completed {
        job_id: String,
        result: Box<CompletionResult>,
    },
}

#[derive(Debug)]
pub struct JobWorker {
    repository: Arc<dyn JobRepository>,
    clock: Arc<dyn ApplicationClock>,
    handler: Arc<dyn JobHandler>,
    worker_id: String,
    lease_duration_ms: i64,
}

impl JobWorker {
    /// Creates one single-kind worker over injected repository and clock ports.
    ///
    /// # Errors
    ///
    /// Returns [`JobError::InvalidClaim`] when its worker or lease policy is invalid.
    pub fn new(
        repository: Arc<dyn JobRepository>,
        clock: Arc<dyn ApplicationClock>,
        handler: Arc<dyn JobHandler>,
        worker_id: String,
        lease_duration_ms: i64,
    ) -> Result<Self, JobError> {
        let claim = ClaimJobs {
            worker_id: worker_id.clone(),
            kinds: BTreeSet::from([handler.kind().to_owned()]),
            now: clock.now(),
            lease_duration_ms,
        };
        claim.validate()?;
        Ok(Self {
            repository,
            clock,
            handler,
            worker_id,
            lease_duration_ms,
        })
    }

    /// Claims and handles at most one currently available job.
    ///
    /// # Errors
    ///
    /// Returns a repository error when claim or durable completion fails.
    pub async fn run_once(&self) -> Result<WorkerStep, JobRepositoryError> {
        let request = ClaimJobs {
            worker_id: self.worker_id.clone(),
            kinds: BTreeSet::from([self.handler.kind().to_owned()]),
            now: self.clock.now(),
            lease_duration_ms: self.lease_duration_ms,
        };
        let Some(lease) = self.repository.claim(request).await? else {
            return Ok(WorkerStep::Idle);
        };
        let outcome = if lease.job.cancellation_requested_at.is_some() {
            JobHandlerOutcome::Cancelled
        } else {
            self.handler.handle(&lease).await
        };
        let job_id = lease.job.job_id.clone();
        let result = self
            .repository
            .complete(lease, outcome, self.clock.now())
            .await?;
        Ok(WorkerStep::Completed {
            job_id,
            result: Box::new(result),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum JobRepositoryError {
    #[error("job not found")]
    NotFound,
    #[error("job identity conflicts with an existing record")]
    Conflict,
    #[error("stored job is corrupt")]
    Corrupt,
    #[error("job repository is unavailable")]
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum JobError {
    #[error("job input is invalid")]
    InvalidJob,
    #[error("claim input is invalid")]
    InvalidClaim,
    #[error("handler outcome is invalid")]
    InvalidOutcome,
}

#[must_use]
pub fn retry_backoff_ms(initial_ms: i64, maximum_ms: i64, attempt: u32) -> i64 {
    let shift = attempt.saturating_sub(1).min(62);
    initial_ms
        .saturating_mul(1_i64.checked_shl(shift).unwrap_or(i64::MAX))
        .min(maximum_ms)
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(&mut output, "{byte:02x}").expect("writing a digest to String cannot fail");
            output
        })
}

fn valid_id(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.trim() == value
        && value.chars().all(|character| !character.is_control())
}

fn valid_kind(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
}

fn valid_error_code(value: &str) -> bool {
    valid_kind(value) && value.len() <= 64
}
