//! `SQLite` persistence adapters for Word Arena.
//!
//! This crate owns database-specific schema and behavior. Application commands
//! and engine rules remain independent from `SQLx` and `SQLite`.

mod agent_repository;
mod capability_repository;
mod job_repository;
mod local_match_repository;
mod migration;
mod rating_repository;
mod repository;
mod scheduler_repository;
mod statistics_repository;
mod tournament_repository;

pub use agent_repository::{
    AgentAttributionError, AgentRunAttribution, AgentRunOutcome, ReplayAgentAttribution,
    SqliteAgentAttributionRepository,
};
pub use capability_repository::SqliteCapabilityRepository;
pub use job_repository::SqliteJobRepository;
pub use local_match_repository::{
    LocalMatchRepositoryError, SqliteLocalMatchRepository, StoredLocalAgentMatch,
};
pub use migration::{MIGRATOR, MigrationError, connect_and_migrate, migrate};
pub use rating_repository::SqliteRatingRepository;
pub use repository::SqliteGameRepository;
pub use scheduler_repository::SqliteSchedulerRepository;
pub use statistics_repository::SqliteStatisticsRepository;
pub use tournament_repository::SqliteTournamentRepository;
