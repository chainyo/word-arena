use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One board coordinate, zero-indexed from the upper-left corner.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Coordinate {
    /// Zero-indexed row.
    pub row: u8,
    /// Zero-indexed column.
    pub column: u8,
}

impl Coordinate {
    /// Creates a coordinate. Bounds are enforced by the selected board.
    #[must_use]
    pub const fn new(row: u8, column: u8) -> Self {
        Self { row, column }
    }

    pub(crate) const fn index(self, width: u8) -> usize {
        self.row as usize * width as usize + self.column as usize
    }

    pub(crate) const fn in_bounds(self, width: u8, height: u8) -> bool {
        self.row < height && self.column < width
    }
}

impl fmt::Display for Coordinate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "({}, {})", self.row, self.column)
    }
}

/// Competitive seat whose score and turn are public.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Seat {
    /// First seat.
    One,
    /// Second seat.
    Two,
    /// Third seat.
    Three,
    /// Fourth seat.
    Four,
}

impl Seat {
    /// Stable seat order.
    pub const ALL: [Self; 4] = [Self::One, Self::Two, Self::Three, Self::Four];
    /// Stable order for paired formats.
    pub const TWO_PLAYER: [Self; 2] = [Self::One, Self::Two];

    /// Zero-based stable seat index.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::One => 0,
            Self::Two => 1,
            Self::Three => 2,
            Self::Four => 3,
        }
    }

    /// One-based stable seat number used by transports and persistence.
    #[must_use]
    pub const fn number(self) -> u8 {
        match self {
            Self::One => 1,
            Self::Two => 2,
            Self::Three => 3,
            Self::Four => 4,
        }
    }

    /// Resolves a one-based seat number.
    #[must_use]
    pub const fn from_number(number: u8) -> Option<Self> {
        match number {
            1 => Some(Self::One),
            2 => Some(Self::Two),
            3 => Some(Self::Three),
            4 => Some(Self::Four),
            _ => None,
        }
    }

    /// Stable active seats for a supported player count.
    #[must_use]
    pub fn active(player_count: usize) -> Option<&'static [Self]> {
        (2..=Self::ALL.len())
            .contains(&player_count)
            .then_some(&Self::ALL[..player_count])
    }

    pub(crate) fn next(self, player_count: usize) -> Option<Self> {
        Self::active(player_count).map(|seats| seats[(self.index() + 1) % seats.len()])
    }
}

/// Backward-compatible name retained while transports adopt seat terminology.
pub type Player = Seat;

/// Canonical token printed on one physical nonblank tile.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct TileToken(String);

impl TileToken {
    /// Creates one canonical `A` through `Z` physical token.
    ///
    /// # Errors
    ///
    /// Returns [`TileTokenError`] for empty, multi-letter, lowercase, accented,
    /// ligature, punctuation, or otherwise nonphysical input.
    pub fn new(token: impl Into<String>) -> Result<Self, TileTokenError> {
        let token = token.into();
        if token.len() == 1 && token.as_bytes()[0].is_ascii_uppercase() {
            Ok(Self(token))
        } else {
            Err(TileTokenError { token })
        }
    }

    /// Canonical display and lookup token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for TileToken {
    type Error = TileTokenError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<TileToken> for String {
    fn from(value: TileToken) -> Self {
        value.0
    }
}

impl fmt::Display for TileToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Invalid physical tile token.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("physical tile token {token:?} must be exactly one uppercase A-Z letter")]
pub struct TileTokenError {
    /// Rejected input.
    pub token: String,
}

/// Stable identity assigned to one physical tile for its whole game lifetime.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct TileId(pub u16);

/// Permanent face printed on a physical tile before any blank assignment.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(tag = "kind", content = "token", rename_all = "snake_case")]
pub enum TileFace {
    /// Scored letter tile.
    Letter(TileToken),
    /// Zero-value wildcard assigned only when placed.
    Blank,
}

/// One uniquely identifiable tile owned by the bag, a rack, or the board.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicalTile {
    /// Stable game-local identity.
    pub id: TileId,
    /// Permanent physical face.
    pub face: TileFace,
}

/// Ordered current tiles owned by one seat.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Rack(Vec<PhysicalTile>);

impl Rack {
    /// Creates a rack from explicitly owned tiles.
    #[must_use]
    pub const fn new(tiles: Vec<PhysicalTile>) -> Self {
        Self(tiles)
    }

    /// Current tiles in stable rack order.
    #[must_use]
    pub fn tiles(&self) -> &[PhysicalTile] {
        &self.0
    }

    /// Current tile count.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the rack contains no tiles.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn extend(&mut self, tiles: impl IntoIterator<Item = PhysicalTile>) {
        self.0.extend(tiles);
    }
}

/// Private ordered future tile source.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Bag(Vec<PhysicalTile>);

impl Bag {
    /// Creates a bag with an already-determined private order.
    #[must_use]
    pub const fn new(tiles: Vec<PhysicalTile>) -> Self {
        Self(tiles)
    }

    /// Current number of undrawn tiles.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether no undrawn tiles remain.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn tiles(&self) -> &[PhysicalTile] {
        &self.0
    }

    pub(crate) fn draw_up_to(&mut self, count: usize) -> Vec<PhysicalTile> {
        let draw_count = count.min(self.0.len());
        let mut drawn = self.0.split_off(self.0.len() - draw_count);
        drawn.reverse();
        drawn
    }
}

/// Immutable multiplier printed on a board square.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Premium {
    /// No multiplier.
    #[default]
    Normal,
    /// Double the newly placed tile value.
    DoubleLetter,
    /// Triple the newly placed tile value.
    TripleLetter,
    /// Double the complete newly formed word.
    DoubleWord,
    /// Triple the complete newly formed word.
    TripleWord,
}

/// One immutable square in a board definition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoardSquare {
    /// Stable square coordinate.
    pub coordinate: Coordinate,
    /// Multiplier available when first covered.
    pub premium: Premium,
}

/// Complete row-major board and premium definition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoardDefinition {
    /// Number of columns.
    pub width: u8,
    /// Number of rows.
    pub height: u8,
    /// Row-major squares, including normal squares.
    pub squares: Vec<BoardSquare>,
}

impl BoardDefinition {
    /// Returns one square when the coordinate is in bounds and the definition
    /// has the expected row-major shape.
    #[must_use]
    pub fn square(&self, coordinate: Coordinate) -> Option<&BoardSquare> {
        coordinate
            .in_bounds(self.width, self.height)
            .then(|| self.squares.get(coordinate.index(self.width)))
            .flatten()
    }
}

/// Checked signed score representation, including endgame deductions.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Score(i32);

impl Score {
    /// Zero score.
    pub const ZERO: Self = Self(0);

    /// Creates an explicit signed score.
    #[must_use]
    pub const fn new(value: i32) -> Self {
        Self(value)
    }

    /// Signed score value.
    #[must_use]
    pub const fn value(self) -> i32 {
        self.0
    }

    /// Checked score adjustment.
    #[must_use]
    pub const fn checked_add(self, delta: i32) -> Option<Self> {
        match self.0.checked_add(delta) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

/// Explicit turn identity recorded with actions and events.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Turn {
    /// Monotonic turn number starting at zero.
    pub number: u64,
    /// Seat authorized to act.
    pub seat: Seat,
}

/// Stable categories for rejected game actions.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Violation {
    /// Caller is not the active seat.
    WrongSeat,
    /// Expected turn or state version is stale.
    StaleTurn,
    /// Tile is not owned by the acting rack.
    TileNotOwned,
    /// Board geometry or connectivity is invalid.
    InvalidPlacement,
    /// One or more formed words are absent from the exact lexicon.
    InvalidWord,
    /// Exchange cannot satisfy ruleset or bag constraints.
    InvalidExchange,
    /// No actions are accepted after completion.
    GameFinished,
}
