//! Transport-agnostic application commands and credential-bound game queries.
//!
//! The application layer coordinates the deterministic engine through injected
//! storage, lexicon, ID, seed, and clock boundaries. HTTP, MCP, `SQLx`, and token
//! parsing are adapters around this crate rather than alternate game rules.

mod authority;
mod capability;
mod command;
mod error;
mod job;
mod operations;
mod ports;
mod rating;
mod scheduler;
mod service;
mod statistics;
mod tournament;

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
    HumanSpectatorReplayQuery, HumanSpectatorReplayView, IdempotencyKey, MovePreviewCommand,
    MovePreviewResult, PublicGameQuery, PublicGameView, SeatGameQuery, SeatGameView,
    TimeoutCommand, UnixMillis,
};
pub use error::{ApplicationError, RepositoryError};
pub use job::{
    CancellationResult, ClaimJobs, CompletionResult, EnqueueResult, JOB_MAX_ATTEMPTS,
    JOB_MAX_BACKOFF_MS, JOB_MAX_LEASE_MS, JOB_PAYLOAD_MAX_BYTES, JOB_SCHEMA_VERSION, JobError,
    JobHandler, JobHandlerOutcome, JobLease, JobRecord, JobRepository, JobRepositoryError,
    JobStatus, JobWorker, NewJob, RenewalResult, WorkerStep, retry_backoff_ms,
};
pub use operations::{
    ACTION_OUTCOME_SCHEMA_VERSION, ActionCommit, ActionOutcome, ActionRejection,
    CreationIdempotencyLookup, CreationIdempotencyRecord, IDEMPOTENCY_DIGEST_VERSION,
    IdempotencyLookup, IdempotencyRecord, InvalidAttemptResponse, InvalidAttemptState,
    OperationalPolicy, PersistedActionResult, PersistedCreateResult, PreviewPolicy, RecoveryRecord,
    TimeoutResponse, TurnDeadline,
};
pub use ports::{
    ApplicationClock, BoxFuture, CapabilityRepository, GameIdSource, GameRepository,
    LexiconResolver, SeedSource, StoredGame,
};
pub use rating::{
    RATING_SCHEMA_VERSION, RatedMatchInput, RatingCommitResult, RatingError, RatingOpponent,
    RatingPeriod, RatingPool, RatingRepository, RatingRepositoryError, RatingUpdateInput,
    RatingValue, SCORE_SCALE, StoredRatingPeriod, update_rating,
};
pub use scheduler::{
    ExecutionReservation, MAX_CONCURRENCY_LIMIT, MAX_RATE_CAPACITY, MAX_RESERVATION_MS, RatePolicy,
    RecoverySnapshot, ReservationRequest, ReservationResult, SCHEDULER_SCHEMA_VERSION,
    SchedulerError, SchedulerRepository, SchedulerRepositoryError, SchedulerScope, SchedulingLimit,
    TerminalCommitResult, TerminalMatchResult, TokenBucketState, TournamentWorkerControl,
    refill_bucket, token_retry_at,
};
pub use service::{ApplicationRuntime, ApplicationService, CapabilityAdapters};
pub use statistics::{
    MatchStatisticsInput, NormalizedRunStatistics, OperatorStatistics, PremiumUse,
    PublicStatistics, RATE_SCALE, STATISTICS_SCHEMA_VERSION, SourcedStatistic,
    StatisticAvailability, StatisticsAccumulator, StatisticsError, StatisticsFilter,
    StatisticsObservation, StatisticsParticipant, StatisticsRecordResult, StatisticsRepository,
    StatisticsRepositoryError, StatisticsScope, aggregate_statistics,
};
pub use tournament::{
    EntrantPairing, ScheduledMatch, ScheduledSeries, SeatBalance, SeriesSeatPolicy,
    StoredTournament, SwissProgress, SwissRematchPolicy, SwissStanding,
    TOURNAMENT_FORMAT_SCHEMA_VERSION, TOURNAMENT_LIFECYCLE_SCHEMA_VERSION,
    TOURNAMENT_SCHEDULE_SCHEMA_VERSION, TournamentBye, TournamentEntrant, TournamentError,
    TournamentFormat, TournamentFormatIdentity, TournamentGameProfile, TournamentLifecycleEvent,
    TournamentLifecycleState, TournamentRepository, TournamentSchedule, TournamentSpec,
};
