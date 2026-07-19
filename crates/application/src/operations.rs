use serde::{Deserialize, Serialize};
use word_arena_engine::{GameEvent, GamePhase, ReplayBundle, Seat, SeatProjection};

use crate::{GameId, StoredGame, UnixMillis};

/// Stable schema for persisted mutation outcomes.
pub const ACTION_OUTCOME_SCHEMA_VERSION: u32 = 1;
/// SHA-256 digest contract for opaque idempotency keys.
pub const IDEMPOTENCY_DIGEST_VERSION: u32 = 1;

/// Deterministic result returned for a rejected mutation and safe to replay.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionRejection {
    /// The supplied optimistic version lost or was already stale.
    VersionConflict,
    /// The submitted move is illegal under the immutable ruleset.
    IllegalAction { message: String },
    /// One key was reused for a different command payload.
    IdempotencyConflict,
    /// A timeout was requested before its persisted deadline.
    DeadlineNotReached,
}

/// Persisted accepted response, independent from a credential instance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PersistedActionResult {
    pub committed_at: UnixMillis,
    pub event: GameEvent,
    pub game: SeatProjection,
}

/// Exact deterministic mutation outcome cached for retry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", content = "value", rename_all = "snake_case")]
pub enum ActionOutcome {
    Accepted(Box<PersistedActionResult>),
    Rejected(ActionRejection),
}

/// Persisted identity and exact outcome for one mutation attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdempotencyRecord {
    pub game_id: GameId,
    pub key_digest: [u8; 32],
    pub digest_version: u32,
    pub command_kind: String,
    pub payload_sha256: String,
    pub outcome: ActionOutcome,
    pub created_at: UnixMillis,
}

/// Lookup result distinguishes a missing key from payload misuse.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdempotencyLookup {
    Missing,
    Match(ActionOutcome),
    PayloadConflict,
}

/// Persisted game-creation response without process-local credentials.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PersistedCreateResult {
    pub game_id: GameId,
    pub created_at: UnixMillis,
    pub public: word_arena_engine::PublicProjection,
}

/// Global creation-key record used before the generated game ID is known.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreationIdempotencyRecord {
    pub key_digest: [u8; 32],
    pub digest_version: u32,
    pub payload_sha256: String,
    pub result: PersistedCreateResult,
}

/// Creation retry lookup result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CreationIdempotencyLookup {
    Missing,
    Match(Box<PersistedCreateResult>),
    PayloadConflict,
}

/// One persisted deadline bound to an exact turn and policy version.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TurnDeadline {
    pub turn: u64,
    pub seat: Seat,
    pub deadline_at: UnixMillis,
    pub policy_version: u32,
}

/// Engine action selected when a turn deadline expires.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeoutResponse {
    Pass,
    Resign,
}

/// Engine action selected after repeated invalid attempts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvalidAttemptResponse {
    RejectOnly,
    Pass,
    Resign,
}

/// Versioned injected reliability policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationalPolicy {
    pub version: u32,
    pub turn_duration_ms: i64,
    pub timeout_response: TimeoutResponse,
    pub invalid_attempt_limit: u32,
    pub invalid_attempt_response: InvalidAttemptResponse,
}

impl Default for OperationalPolicy {
    fn default() -> Self {
        Self {
            version: 1,
            turn_duration_ms: 300_000,
            timeout_response: TimeoutResponse::Pass,
            invalid_attempt_limit: 3,
            invalid_attempt_response: InvalidAttemptResponse::RejectOnly,
        }
    }
}

/// Persisted counter scoped to one exact turn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidAttemptState {
    pub turn: u64,
    pub seat: Seat,
    pub policy_version: u32,
    pub count: u32,
}

/// Atomic repository write for either an accepted transition or rejection.
#[derive(Clone, Debug)]
pub struct ActionCommit {
    pub expected_version: u64,
    pub successor: Option<StoredGame>,
    pub idempotency: IdempotencyRecord,
    pub invalid_attempt: Option<InvalidAttemptState>,
    pub replay: Option<ReplayBundle>,
}

impl ActionCommit {
    #[must_use]
    pub fn outcome_kind(&self) -> &'static str {
        match self.idempotency.outcome {
            ActionOutcome::Accepted(_) => "accepted",
            ActionOutcome::Rejected(_) => "rejected",
        }
    }
}

/// Persisted recovery artifact available only when the game is finished.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryRecord {
    pub game_id: GameId,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
    pub phase: GamePhase,
    pub replay: ReplayBundle,
}
