//! Transport-agnostic application commands and authority-bound game queries.
//!
//! The application layer coordinates the deterministic engine through injected
//! storage, lexicon, ID, seed, and clock boundaries. HTTP, MCP, `SQLx`, and token
//! parsing are adapters around this crate rather than alternate game rules.

mod authority;
mod command;
mod error;
mod ports;
mod service;

#[cfg(feature = "test-support")]
pub mod test_support;

pub use authority::{
    AdministratorAuthority, CreatedGameAccess, HumanSpectatorAuthority, SeatAuthority,
};
pub use command::{
    AdministratorGameQuery, AdministratorGameView, CreateGameCommand, CreatedGame,
    GameActionCommand, GameActionResult, GameId, HumanSpectatorGameQuery, HumanSpectatorGameView,
    IdempotencyKey, PublicGameQuery, PublicGameView, SeatGameQuery, SeatGameView, UnixMillis,
};
pub use error::{ApplicationError, RepositoryError};
pub use ports::{
    ApplicationClock, BoxFuture, GameIdSource, GameRepository, LexiconResolver, SeedSource,
    StoredGame,
};
pub use service::ApplicationService;
