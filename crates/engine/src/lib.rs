//! Deterministic game domain and rules engine for Word Arena.
//!
//! Transport, persistence, authentication, clocks, IDs, random tile sources,
//! and pack installation belong outside this crate. The engine receives one
//! already verified immutable lexicon through a query-only boundary.

mod error;
mod game;
mod language;
mod lexicon;
mod model;
mod random;
mod ruleset;

/// Deterministic baseline bots and an in-memory runner for engine verification.
///
/// This module is deliberately absent from default builds so competitive HTTP
/// and MCP transports cannot accidentally expose a best-move surface.
#[cfg(feature = "test-support")]
pub mod test_support;

pub use error::GameError;
pub use game::{
    AdministratorProjection, BOARD_SIZE, BoardTile, EventVisibility, FormedWord, Game, GameEvent,
    GameEventKind, GameMode, GamePhase, GameResult, GameSnapshot, HumanSpectatorProjection, Move,
    PROJECTION_SCHEMA_VERSION, PUBLIC_REPLAY_SCHEMA_VERSION, Placement, PrivateGameEvent,
    PublicGameState, PublicProjection, PublicReplayBundle, REPLAY_SCHEMA_VERSION, ReplayBundle,
    SNAPSHOT_SCHEMA_VERSION, SeatProjection, TerminalReason, Tile,
};
pub use language::Language;
pub use lexicon::WordValidator;
pub use model::{
    Bag, BoardDefinition, BoardSquare, Coordinate, PhysicalTile, Player, Premium, Rack, Score,
    Seat, TileFace, TileId, TileToken, TileTokenError, Turn, Violation,
};
pub use random::{
    ConservationError, GameSeed, InitialDeal, RandomError, RngAlgorithm, SeedCommitment,
    prepare_initial_deal, prepare_initial_deal_for_players, verify_tile_conservation,
};
pub use ruleset::{
    GameRules, RULESET_SCHEMA_VERSION, Ruleset, RulesetDefinitionError, RulesetFixtureError,
    RulesetId, RulesetIdentity, TileDefinition,
};
