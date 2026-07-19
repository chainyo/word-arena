use std::{fmt::Debug, future::Future, pin::Pin, sync::Arc};

use word_arena_engine::{GameSeed, GameSnapshot, WordValidator};
use word_arena_lexicon::{PackIdentity, PackManifest};

use crate::{
    ActionCommit, AuditRecord, CapabilityId, CapabilityRecord, CapabilityRepositoryError,
    CreationIdempotencyLookup, CreationIdempotencyRecord, GameId, IdempotencyLookup,
    InvalidAttemptState, RecoveryRecord, RepositoryError, TurnDeadline, UnixMillis,
};

/// Sendable boxed future used by adapter ports without an async-trait macro.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Persistable application record independent from any storage technology.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredGame {
    /// Stable application game ID.
    pub game_id: GameId,
    /// Injected creation time.
    pub created_at: UnixMillis,
    /// Injected time of the latest committed version.
    pub updated_at: UnixMillis,
    /// Complete authoritative engine checkpoint.
    pub snapshot: GameSnapshot,
    /// Persisted deadline for the current active turn.
    pub turn_deadline: Option<TurnDeadline>,
}

/// Game persistence boundary implemented in memory for tests and by `SQLx` for
/// production `SQLite` storage.
pub trait GameRepository: Debug + Send + Sync {
    /// Inserts a game exactly once.
    fn insert(&self, game: StoredGame) -> BoxFuture<'_, Result<(), RepositoryError>>;

    /// Inserts a game and its global creation retry outcome atomically.
    fn insert_idempotent(
        &self,
        game: StoredGame,
        idempotency: CreationIdempotencyRecord,
    ) -> BoxFuture<'_, Result<(), RepositoryError>>;

    /// Looks up a creation retry before allocating or exposing a new game.
    fn load_creation_idempotency(
        &self,
        key_digest: [u8; 32],
        payload_sha256: &str,
    ) -> BoxFuture<'_, Result<CreationIdempotencyLookup, RepositoryError>>;

    /// Loads one complete authoritative game record.
    fn load(&self, game_id: &GameId) -> BoxFuture<'_, Result<StoredGame, RepositoryError>>;

    /// Replaces one checkpoint only when its persisted version matches.
    fn replace(
        &self,
        expected_version: u64,
        game: StoredGame,
    ) -> BoxFuture<'_, Result<(), RepositoryError>>;

    /// Looks up one retry key while verifying the original payload identity.
    fn load_idempotency(
        &self,
        game_id: &GameId,
        key_digest: [u8; 32],
        payload_sha256: &str,
    ) -> BoxFuture<'_, Result<IdempotencyLookup, RepositoryError>>;

    /// Loads the invalid-attempt counter for one exact turn.
    fn load_invalid_attempt(
        &self,
        game_id: &GameId,
        turn: u64,
        seat: word_arena_engine::Seat,
    ) -> BoxFuture<'_, Result<Option<InvalidAttemptState>, RepositoryError>>;

    /// Atomically records an outcome and any associated game transition.
    fn commit_action(&self, commit: ActionCommit) -> BoxFuture<'_, Result<(), RepositoryError>>;

    /// Loads a finished-game replay artifact for corrupt-snapshot recovery.
    fn load_recovery(
        &self,
        game_id: &GameId,
    ) -> BoxFuture<'_, Result<RecoveryRecord, RepositoryError>>;

    /// Lists persisted deadlines that are currently due, bounded for workers.
    fn due_timeouts(
        &self,
        now: UnixMillis,
        limit: u32,
    ) -> BoxFuture<'_, Result<Vec<crate::TimeoutCommand>, RepositoryError>>;
}

/// Capability and privacy-safe audit persistence boundary.
pub trait CapabilityRepository: Debug + Send + Sync {
    /// Inserts one capability and its issuance audit atomically.
    fn insert(
        &self,
        capability: CapabilityRecord,
        audit: AuditRecord,
    ) -> BoxFuture<'_, Result<(), CapabilityRepositoryError>>;

    /// Loads one record by its public capability ID.
    fn load(
        &self,
        capability_id: &CapabilityId,
    ) -> BoxFuture<'_, Result<CapabilityRecord, CapabilityRepositoryError>>;

    /// Revokes one active capability and appends its audit atomically.
    fn revoke(
        &self,
        capability_id: &CapabilityId,
        revoked_at: UnixMillis,
        audit: AuditRecord,
    ) -> BoxFuture<'_, Result<(), CapabilityRepositoryError>>;

    /// Replaces one active capability and appends rotation audits atomically.
    fn rotate(
        &self,
        prior_id: &CapabilityId,
        revoked_at: UnixMillis,
        replacement: CapabilityRecord,
        audits: [AuditRecord; 2],
    ) -> BoxFuture<'_, Result<(), CapabilityRepositoryError>>;

    /// Appends one authentication or privileged-access audit record.
    fn append_audit(
        &self,
        audit: AuditRecord,
    ) -> BoxFuture<'_, Result<(), CapabilityRepositoryError>>;
}

/// Exact immutable lexicon lookup resolver.
pub trait LexiconResolver: Debug + Send + Sync {
    /// Resolves only the complete requested pack identity.
    fn resolve(&self, identity: &PackIdentity) -> Option<Arc<dyn WordValidator>>;

    /// Returns the verified immutable manifest for an exact installed pack.
    ///
    /// Adapters that provide only a test validator may leave metadata
    /// unavailable; production resolvers should return the same manifest that
    /// was verified before exposing the validator.
    fn manifest(&self, _identity: &PackIdentity) -> Option<PackManifest> {
        None
    }
}

/// Collision-resistant application game ID source.
pub trait GameIdSource: Debug + Send + Sync {
    /// Produces the next fully validated game ID.
    fn next_game_id(&self) -> GameId;
}

/// Private deterministic engine-seed source.
pub trait SeedSource: Debug + Send + Sync {
    /// Produces the next 256-bit game seed.
    fn next_seed(&self) -> GameSeed;
}

/// UTC application clock; the engine remains clock-free.
pub trait ApplicationClock: Debug + Send + Sync {
    /// Current Unix time in milliseconds.
    fn now(&self) -> UnixMillis;
}
