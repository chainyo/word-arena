use std::{fmt::Debug, future::Future, pin::Pin, sync::Arc};

use word_arena_engine::{GameSeed, GameSnapshot, WordValidator};
use word_arena_lexicon::PackIdentity;

use crate::{
    AuditRecord, CapabilityId, CapabilityRecord, CapabilityRepositoryError, GameId,
    RepositoryError, UnixMillis,
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
}

/// Game persistence boundary implemented in memory for tests and by `SQLx` for
/// production `SQLite` storage.
pub trait GameRepository: Debug + Send + Sync {
    /// Inserts a game exactly once.
    fn insert(&self, game: StoredGame) -> BoxFuture<'_, Result<(), RepositoryError>>;

    /// Loads one complete authoritative game record.
    fn load(&self, game_id: &GameId) -> BoxFuture<'_, Result<StoredGame, RepositoryError>>;

    /// Replaces one checkpoint only when its persisted version matches.
    fn replace(
        &self,
        expected_version: u64,
        game: StoredGame,
    ) -> BoxFuture<'_, Result<(), RepositoryError>>;
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
