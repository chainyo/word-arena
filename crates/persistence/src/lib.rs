//! `SQLite` persistence adapters for Word Arena.
//!
//! This crate owns database-specific schema and behavior. Application commands
//! and engine rules remain independent from `SQLx` and `SQLite`.

mod migration;
mod repository;

pub use migration::{MIGRATOR, MigrationError, connect_and_migrate, migrate};
pub use repository::SqliteGameRepository;
