use thiserror::Error;
use word_arena_lexicon::{CompatibilityError, NormalizedKeyError, PackIdentity};

use crate::{Coordinate, Language, Player, RulesetId};

/// Deterministic rules, lexicon, resume, and replay failures.
#[derive(Debug, Error)]
pub enum GameError {
    /// A language does not yet have an approved offline ruleset.
    #[error("no curated offline V1 ruleset is available for language {language:?}")]
    RulesetUnavailable {
        /// Unsupported language.
        language: Language,
    },

    /// A decoded or caller-constructed ruleset differs from its static V1 definition.
    #[error("ruleset {ruleset:?} does not match its immutable built-in definition")]
    InvalidRuleset {
        /// Tampered or unsupported ruleset ID.
        ruleset: RulesetId,
    },

    /// The required immutable pack was not supplied.
    #[error(
        "ruleset {ruleset:?} requires lexicon {required}, but that exact pack is unavailable; run `cargo xtask setup`"
    )]
    MissingLexicon {
        /// Ruleset being started or restored.
        ruleset: RulesetId,
        /// Exact required identity.
        required: Box<PackIdentity>,
    },

    /// Pack identity differs from the immutable rules/game/replay pin.
    #[error(transparent)]
    IncompatibleLexicon(#[from] CompatibilityError),

    /// A persisted ruleset does not match the supplied rules.
    #[error("persisted game requires ruleset {expected:?}, but {actual:?} was supplied")]
    RulesetMismatch {
        /// Persisted ruleset.
        expected: RulesetId,
        /// Supplied ruleset.
        actual: RulesetId,
    },

    /// Persisted state uses an unsupported schema.
    #[error("unsupported {artifact} schema version {found}; expected {expected}")]
    UnsupportedSchema {
        /// Snapshot or replay.
        artifact: &'static str,
        /// Persisted value.
        found: u32,
        /// Implemented value.
        expected: u32,
    },

    /// A move was attempted by the wrong seat.
    #[error("it is {expected:?}'s turn, not {actual:?}'s")]
    WrongPlayer {
        /// Active player.
        expected: Player,
        /// Caller.
        actual: Player,
    },

    /// No tiles were placed.
    #[error("a tile placement must contain at least one tile")]
    EmptyPlacement,

    /// One coordinate is outside the fixed 15-square board.
    #[error("coordinate {coordinate} is outside the 15x15 board")]
    CoordinateOutOfBounds {
        /// Rejected square.
        coordinate: Coordinate,
    },

    /// More than one new tile targets the same square.
    #[error("placement contains coordinate {coordinate} more than once")]
    DuplicateCoordinate {
        /// Repeated square.
        coordinate: Coordinate,
    },

    /// A new tile targets an occupied square.
    #[error("board square {coordinate} is already occupied")]
    OccupiedSquare {
        /// Occupied square.
        coordinate: Coordinate,
    },

    /// New tiles are not collinear.
    #[error("all newly placed tiles must share one row or one column")]
    NotAligned,

    /// An aligned placement contains an unfilled gap.
    #[error("placement is not contiguous at {coordinate}")]
    NotContiguous {
        /// First empty gap.
        coordinate: Coordinate,
    },

    /// Opening move does not cover the center.
    #[error("the opening placement must cover the center square")]
    OpeningMoveMissesCenter,

    /// Later move does not touch the existing board.
    #[error("placement must connect to at least one existing tile")]
    DisconnectedPlacement,

    /// Placement forms no word of two or more tiles.
    #[error("placement must form at least one word of two or more tiles")]
    NoWordFormed,

    /// A tile assignment cannot use the ruleset normalization profile.
    #[error("tile or word normalization failed: {0}")]
    Normalization(#[from] NormalizedKeyError),

    /// Main or cross word is absent from the exact pack.
    #[error("word {word:?} normalizes to {normalized:?} and is not in the active lexicon")]
    InvalidWord {
        /// Board spelling.
        word: String,
        /// Exact queried key.
        normalized: String,
    },

    /// Defensive checked arithmetic rejected an impossible V1 score total.
    #[error("move score exceeds the supported u32 range")]
    ScoreOverflow,

    /// Defensive checked arithmetic rejected an exhausted event sequence.
    #[error("game version exceeds the supported u64 range")]
    VersionOverflow,

    /// Finished games cannot accept another mutation.
    #[error("the game is already finished")]
    GameFinished,

    /// Snapshot board representation is malformed.
    #[error("snapshot board contains {actual} squares; expected {expected}")]
    InvalidSnapshotBoard {
        /// Persisted square count.
        actual: usize,
        /// Fixed board count.
        expected: usize,
    },

    /// Replay event differs from deterministic recomputation.
    #[error("replay event #{sequence} does not match deterministic recomputation")]
    ReplayEventMismatch {
        /// Event sequence.
        sequence: u64,
    },

    /// Replay event ordering or shape is invalid.
    #[error("invalid replay event #{sequence}: {reason}")]
    InvalidReplayEvent {
        /// Event sequence.
        sequence: u64,
        /// Required ordering rule.
        reason: &'static str,
    },
}
