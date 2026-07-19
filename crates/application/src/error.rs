use thiserror::Error;
use word_arena_engine::{GameError, Seat};

use crate::{ActionRejection, GameId};

/// Stable storage error categories safe for application matching.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RepositoryError {
    /// The requested game does not exist.
    #[error("game not found")]
    NotFound,
    /// The game ID already exists.
    #[error("game already exists")]
    AlreadyExists,
    /// The expected version lost an optimistic concurrency race.
    #[error("game version conflict")]
    Conflict,
    /// Stored bytes or relationships are invalid.
    #[error("stored game is corrupt")]
    Corrupt,
    /// Stored schema is valid but unsupported by this binary.
    #[error("stored game schema is incompatible")]
    IncompatibleSchema,
    /// Stored exact lexicon identity is inconsistent or unavailable.
    #[error("stored game lexicon pack is incompatible")]
    IncompatiblePack,
    /// Adapter cannot currently complete the operation.
    #[error("game repository is unavailable")]
    Unavailable,
}

/// Application use-case failure without transport or database details.
#[derive(Debug, Error)]
pub enum ApplicationError {
    /// Game ID violates the public identifier contract.
    #[error("game ID must be 1-128 non-whitespace printable ASCII bytes")]
    InvalidGameId,
    /// Idempotency key violates the public identifier contract.
    #[error("idempotency key must be 1-256 non-whitespace printable ASCII bytes")]
    InvalidIdempotencyKey,
    /// Exact required lexicon is unavailable.
    #[error("exact lexicon pack is unavailable for game {game_id}")]
    MissingLexicon {
        /// Affected game.
        game_id: GameId,
    },
    /// Authority belongs to another game.
    #[error("authority is not valid for game {game_id}")]
    WrongGameAuthority {
        /// Requested game.
        game_id: GameId,
    },
    /// Turn claims a seat other than the bound seat.
    #[error("seat {actual:?} cannot issue a turn for {claimed:?}")]
    WrongSeatAuthority {
        /// Bound authority seat.
        actual: Seat,
        /// Seat claimed by the command turn.
        claimed: Seat,
    },
    /// Turn number and optimistic version differ.
    #[error("turn number {turn} differs from expected version {expected_version}")]
    TurnVersionMismatch {
        /// Command turn number.
        turn: u64,
        /// Command expected version.
        expected_version: u64,
    },
    /// Move preview was requested for a competitive game.
    #[error("move preview is available only in explicit practice games")]
    PracticeOnly,
    /// One seat exhausted its versioned fixed-window preview allowance.
    #[error("move preview rate limit reached; retry after {retry_after_ms} ms")]
    PreviewRateLimited {
        /// Remaining injected-clock duration in the active window.
        retry_after_ms: i64,
    },
    /// The in-process preview limiter could not safely access its state.
    #[error("move preview limiter is unavailable")]
    PreviewUnavailable,
    /// Finished-game replay was requested before the game reached a terminal state.
    #[error("replay is available only after the game is finished")]
    ReplayNotReady,
    /// Stable deterministic mutation rejection, including cached retries.
    #[error("action rejected: {0:?}")]
    ActionRejected(ActionRejection),
    /// Repository failure mapped to a stable category.
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    /// Deterministic engine validation failure.
    #[error(transparent)]
    Engine(#[from] GameError),
}
