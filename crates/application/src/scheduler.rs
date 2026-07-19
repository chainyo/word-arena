use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{BoxFuture, UnixMillis};

pub const SCHEDULER_SCHEMA_VERSION: u32 = 1;
pub const MAX_CONCURRENCY_LIMIT: u32 = 10_000;
pub const MAX_RATE_CAPACITY: u32 = 1_000_000;
pub const MAX_RESERVATION_MS: i64 = 86_400_000;

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum SchedulerScope {
    Global,
    Tournament(String),
    Harness(String),
    Provider(String),
}

impl SchedulerScope {
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::Global => "global".to_owned(),
            Self::Tournament(id) => format!("tournament:{id}"),
            Self::Harness(id) => format!("harness:{id}"),
            Self::Provider(id) => format!("provider:{id}"),
        }
    }

    fn validate(&self) -> bool {
        match self {
            Self::Global => true,
            Self::Tournament(id) | Self::Harness(id) | Self::Provider(id) => valid_id(id, 128),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RatePolicy {
    pub capacity: u32,
    pub refill_tokens: u32,
    pub refill_interval_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulingLimit {
    pub schema_version: u32,
    pub scope: SchedulerScope,
    pub max_concurrency: u32,
    pub rate: Option<RatePolicy>,
}

impl SchedulingLimit {
    /// Validates one versioned concurrency and optional token-bucket policy.
    ///
    /// # Errors
    ///
    /// Returns [`SchedulerError::InvalidInput`] for unsafe bounds or identifiers.
    pub fn validate(&self) -> Result<(), SchedulerError> {
        if self.schema_version != SCHEDULER_SCHEMA_VERSION
            || !self.scope.validate()
            || !(1..=MAX_CONCURRENCY_LIMIT).contains(&self.max_concurrency)
            || self.rate.as_ref().is_some_and(|rate| {
                rate.capacity == 0
                    || rate.capacity > MAX_RATE_CAPACITY
                    || rate.refill_tokens == 0
                    || rate.refill_tokens > MAX_RATE_CAPACITY
                    || rate.refill_interval_ms <= 0
                    || rate.refill_interval_ms > 86_400_000
            })
        {
            Err(SchedulerError::InvalidInput)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TournamentWorkerControl {
    Running,
    Paused,
    Draining,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReservationRequest {
    pub schema_version: u32,
    pub reservation_id: String,
    pub job_id: String,
    pub tournament_id: String,
    pub match_id: String,
    pub run_id: String,
    pub harness_id: String,
    pub provider_id: String,
    pub immutable_inputs_sha256: String,
    pub owner: String,
    pub now: UnixMillis,
    pub duration_ms: i64,
}

impl ReservationRequest {
    /// Validates immutable execution identity, owner, time, and lease bounds.
    ///
    /// # Errors
    ///
    /// Returns [`SchedulerError::InvalidInput`] for malformed requests.
    pub fn validate(&self) -> Result<(), SchedulerError> {
        if self.schema_version != SCHEDULER_SCHEMA_VERSION
            || [
                &self.reservation_id,
                &self.job_id,
                &self.tournament_id,
                &self.match_id,
                &self.run_id,
                &self.harness_id,
                &self.provider_id,
                &self.owner,
            ]
            .into_iter()
            .any(|value| !valid_id(value, 256))
            || !valid_sha256(&self.immutable_inputs_sha256)
            || self.now.0 < 0
            || !(1..=MAX_RESERVATION_MS).contains(&self.duration_ms)
        {
            Err(SchedulerError::InvalidInput)
        } else {
            Ok(())
        }
    }

    #[must_use]
    pub fn scopes(&self) -> [SchedulerScope; 4] {
        [
            SchedulerScope::Global,
            SchedulerScope::Tournament(self.tournament_id.clone()),
            SchedulerScope::Harness(self.harness_id.clone()),
            SchedulerScope::Provider(self.provider_id.clone()),
        ]
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionReservation {
    pub request: ReservationRequest,
    pub expires_at: UnixMillis,
    pub cancellation_requested: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReservationResult {
    Acquired(Box<ExecutionReservation>),
    Limited { retry_at: Option<UnixMillis> },
    Paused,
    Draining,
    Cancelled,
    AlreadyFinished,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalMatchResult {
    pub schema_version: u32,
    pub match_id: String,
    pub immutable_inputs_sha256: String,
    pub result_sha256: String,
    pub charge_key: String,
    pub telemetry_key: String,
    pub rating_key: String,
}

impl TerminalMatchResult {
    /// Validates immutable result and downstream idempotency identities.
    ///
    /// # Errors
    ///
    /// Returns [`SchedulerError::InvalidInput`] for malformed identities.
    pub fn validate(&self) -> Result<(), SchedulerError> {
        if self.schema_version == SCHEDULER_SCHEMA_VERSION
            && valid_id(&self.match_id, 256)
            && valid_sha256(&self.immutable_inputs_sha256)
            && valid_sha256(&self.result_sha256)
            && valid_id(&self.charge_key, 256)
            && valid_id(&self.telemetry_key, 256)
            && valid_id(&self.rating_key, 256)
        {
            Ok(())
        } else {
            Err(SchedulerError::InvalidInput)
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalCommitResult {
    Applied(TerminalMatchResult),
    AlreadyApplied(TerminalMatchResult),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoverySnapshot {
    pub controls: BTreeMap<String, TournamentWorkerControl>,
    pub active: Vec<ExecutionReservation>,
    pub expired_match_ids: Vec<String>,
}

pub trait SchedulerRepository: std::fmt::Debug + Send + Sync {
    fn configure(
        &self,
        limits: Vec<SchedulingLimit>,
        now: UnixMillis,
    ) -> BoxFuture<'_, Result<(), SchedulerRepositoryError>>;
    fn set_control<'a>(
        &'a self,
        tournament_id: &'a str,
        control: TournamentWorkerControl,
        now: UnixMillis,
    ) -> BoxFuture<'a, Result<(), SchedulerRepositoryError>>;
    fn acquire(
        &self,
        request: ReservationRequest,
    ) -> BoxFuture<'_, Result<ReservationResult, SchedulerRepositoryError>>;
    fn renew(
        &self,
        reservation: ExecutionReservation,
        now: UnixMillis,
        duration_ms: i64,
    ) -> BoxFuture<'_, Result<ExecutionReservation, SchedulerRepositoryError>>;
    fn release(
        &self,
        reservation: ExecutionReservation,
        now: UnixMillis,
    ) -> BoxFuture<'_, Result<(), SchedulerRepositoryError>>;
    fn commit_terminal(
        &self,
        reservation: ExecutionReservation,
        result: TerminalMatchResult,
        now: UnixMillis,
    ) -> BoxFuture<'_, Result<TerminalCommitResult, SchedulerRepositoryError>>;
    fn reconstruct(
        &self,
        now: UnixMillis,
    ) -> BoxFuture<'_, Result<RecoverySnapshot, SchedulerRepositoryError>>;
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum SchedulerRepositoryError {
    #[error("scheduler state conflicts with the requested transition")]
    Conflict,
    #[error("scheduler state is corrupt")]
    Corrupt,
    #[error("scheduler repository is unavailable")]
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum SchedulerError {
    #[error("scheduler input is invalid")]
    InvalidInput,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TokenBucketState {
    pub tokens: u32,
    pub remainder: u64,
    pub updated_at: UnixMillis,
}

/// Refills one deterministic integer token bucket without floating point.
///
/// # Errors
///
/// Returns [`SchedulerError::InvalidInput`] for clock reversal or invalid policy.
pub fn refill_bucket(
    state: TokenBucketState,
    policy: &RatePolicy,
    now: UnixMillis,
) -> Result<TokenBucketState, SchedulerError> {
    if now < state.updated_at
        || policy.capacity == 0
        || policy.refill_tokens == 0
        || policy.refill_interval_ms <= 0
    {
        return Err(SchedulerError::InvalidInput);
    }
    if state.tokens >= policy.capacity {
        return Ok(TokenBucketState {
            tokens: policy.capacity,
            remainder: 0,
            updated_at: now,
        });
    }
    let elapsed =
        u128::try_from(now.0 - state.updated_at.0).map_err(|_| SchedulerError::InvalidInput)?;
    let numerator = elapsed
        .saturating_mul(u128::from(policy.refill_tokens))
        .saturating_add(u128::from(state.remainder));
    let interval =
        u128::try_from(policy.refill_interval_ms).map_err(|_| SchedulerError::InvalidInput)?;
    let added = u32::try_from(numerator / interval).unwrap_or(u32::MAX);
    let tokens = state.tokens.saturating_add(added).min(policy.capacity);
    Ok(TokenBucketState {
        tokens,
        remainder: if tokens == policy.capacity {
            0
        } else {
            u64::try_from(numerator % interval).map_err(|_| SchedulerError::InvalidInput)?
        },
        updated_at: now,
    })
}

#[must_use]
pub fn token_retry_at(
    state: TokenBucketState,
    policy: &RatePolicy,
    now: UnixMillis,
) -> Option<UnixMillis> {
    if state.tokens > 0 {
        return Some(now);
    }
    let remaining = u64::try_from(policy.refill_interval_ms)
        .ok()?
        .saturating_sub(state.remainder);
    let refill = u64::from(policy.refill_tokens);
    let delay = remaining.saturating_add(refill.saturating_sub(1)) / refill;
    now.0
        .checked_add(i64::try_from(delay).ok()?)
        .map(UnixMillis)
}

fn valid_id(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.trim() == value
        && value.chars().all(|character| !character.is_control())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
