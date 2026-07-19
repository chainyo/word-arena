//! Transport-agnostic application commands and credential-bound game queries.
//!
//! The application layer coordinates the deterministic engine through injected
//! storage, lexicon, ID, seed, and clock boundaries. HTTP, MCP, `SQLx`, and token
//! parsing are adapters around this crate rather than alternate game rules.

mod authority;
mod capability;
mod command;
mod error;
mod operations;
mod ports;
mod service;

#[cfg(feature = "test-support")]
pub mod test_support;

pub use authority::{
    AdministratorCredential, Authorizes, CompetitiveGameCredentials, CompetitiveSeatCredential,
    CreatedGameAccess, HumanSpectatorCredential, PublicViewerCredential,
};
pub use capability::{
    AgentRunId, AuditAction, AuditActor, AuditOutcome, AuditRecord, AuthenticatedCredential,
    CAPABILITY_DIGEST_VERSION, CapabilityDescriptor, CapabilityDigestKey, CapabilityError,
    CapabilityId, CapabilityMaterial, CapabilityRecord, CapabilityRepositoryError, CapabilityRole,
    CapabilityScope, CapabilityToken, CapabilityTokenSource, IssueCapabilityRequest,
    IssuedCapability, SystemCapabilityTokenSource,
};
pub use command::{
    AdministratorGameQuery, AdministratorGameView, CreateGameCommand, CreatedGame,
    GameActionCommand, GameActionResult, GameId, HumanSpectatorGameQuery, HumanSpectatorGameView,
    IdempotencyKey, PublicGameQuery, PublicGameView, SeatGameQuery, SeatGameView, TimeoutCommand,
    UnixMillis,
};
pub use error::{ApplicationError, RepositoryError};
pub use operations::{
    ACTION_OUTCOME_SCHEMA_VERSION, ActionCommit, ActionOutcome, ActionRejection,
    CreationIdempotencyLookup, CreationIdempotencyRecord, IDEMPOTENCY_DIGEST_VERSION,
    IdempotencyLookup, IdempotencyRecord, InvalidAttemptResponse, InvalidAttemptState,
    OperationalPolicy, PersistedActionResult, PersistedCreateResult, RecoveryRecord,
    TimeoutResponse, TurnDeadline,
};
pub use ports::{
    ApplicationClock, BoxFuture, CapabilityRepository, GameIdSource, GameRepository,
    LexiconResolver, SeedSource, StoredGame,
};
pub use service::{ApplicationRuntime, ApplicationService, CapabilityAdapters};
