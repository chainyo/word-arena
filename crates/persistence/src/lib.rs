//! `SQLite` persistence adapters for Word Arena.
//!
//! This crate owns database-specific schema and behavior. Application commands
//! and engine rules remain independent from `SQLx` and `SQLite`.

mod agent_repository;
mod capability_repository;
mod migration;
mod repository;

pub use agent_repository::{
    AgentAttributionError, AgentRunAttribution, AgentRunOutcome, ReplayAgentAttribution,
    SqliteAgentAttributionRepository,
};
pub use capability_repository::SqliteCapabilityRepository;
pub use migration::{MIGRATOR, MigrationError, connect_and_migrate, migrate};
pub use repository::SqliteGameRepository;
