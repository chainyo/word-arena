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

pub use error::GameError;
pub use game::{
    AdministratorProjection, BOARD_SIZE, BoardTile, EventVisibility, FormedWord, Game, GameEvent,
    GameEventKind, GamePhase, GameResult, GameSnapshot, HumanSpectatorProjection, Move, Placement,
    PrivateGameEvent, PublicGameState, PublicProjection, ReplayBundle, SeatProjection,
    TerminalReason, Tile,
};
pub use language::Language;
pub use lexicon::WordValidator;
pub use model::{
    Bag, BoardDefinition, BoardSquare, Coordinate, PhysicalTile, Player, Premium, Rack, Score,
    Seat, TileFace, TileId, TileToken, TileTokenError, Turn, Violation,
};
pub use random::{
    ConservationError, GameSeed, InitialDeal, RandomError, RngAlgorithm, SeedCommitment,
    prepare_initial_deal, verify_tile_conservation,
};
pub use ruleset::{
    GameRules, RULESET_SCHEMA_VERSION, Ruleset, RulesetDefinitionError, RulesetFixtureError,
    RulesetId, RulesetIdentity, TileDefinition,
};
