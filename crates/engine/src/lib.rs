//! Deterministic game domain and rules engine for Word Arena.
//!
//! Transport, persistence, authentication, clocks, IDs, random tile sources,
//! and pack installation belong outside this crate. The engine receives one
//! already verified immutable lexicon through a query-only boundary.

mod error;
mod game;
mod language;
mod lexicon;
mod ruleset;

pub use error::GameError;
pub use game::{
    BOARD_SIZE, BoardTile, Coordinate, FormedWord, Game, GameEvent, GameEventKind, GamePhase,
    GameResult, GameSnapshot, Placement, Player, PublicGameState, ReplayBundle, Tile,
};
pub use language::Language;
pub use lexicon::WordValidator;
pub use ruleset::{RULESET_SCHEMA_VERSION, Ruleset, RulesetId};
