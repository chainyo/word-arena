use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use word_arena_lexicon::{CompatibilityContext, PackIdentity, normalize_key};

use crate::{Coordinate, GameError, Player, Ruleset, RulesetId, TileId, WordValidator};

/// Width and height of the V1 board.
pub const BOARD_SIZE: u8 = 15;
const BOARD_SQUARES: usize = BOARD_SIZE as usize * BOARD_SIZE as usize;
const CENTER: Coordinate = Coordinate { row: 7, column: 7 };
const SNAPSHOT_SCHEMA_VERSION: u32 = 1;
const REPLAY_SCHEMA_VERSION: u32 = 1;

/// A tile assignment supplied by a player.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Tile {
    /// Physical board token. Accepted input is canonicalized to one A-Z letter.
    pub letter: String,
    /// Whether this is a zero-point blank tile.
    pub is_blank: bool,
}

impl Tile {
    /// Creates a regular scored tile.
    #[must_use]
    pub fn letter(letter: impl Into<String>) -> Self {
        Self {
            letter: letter.into(),
            is_blank: false,
        }
    }

    /// Creates a zero-point blank assigned to one physical board letter.
    #[must_use]
    pub fn blank(assigned_letter: impl Into<String>) -> Self {
        Self {
            letter: assigned_letter.into(),
            is_blank: true,
        }
    }
}

/// One new tile and its target square.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    /// Target square.
    pub coordinate: Coordinate,
    /// Tile and blank assignment, canonicalized before a move is committed.
    pub tile: Tile,
}

/// Typed player action accepted by the complete game engine.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Move {
    /// Place one or more rack tiles on the board.
    Place {
        /// Proposed square assignments.
        placements: Vec<Placement>,
    },
    /// Return owned tiles and draw deterministic replacements.
    Exchange {
        /// Stable IDs selected from the acting rack.
        tile_ids: Vec<TileId>,
    },
    /// End the turn without changing tiles or score.
    Pass,
    /// End the game immediately.
    Resign,
}

impl Placement {
    /// Creates a placement value.
    #[must_use]
    pub const fn new(coordinate: Coordinate, tile: Tile) -> Self {
        Self { coordinate, tile }
    }
}

/// Immutable tile stored on the public board.
pub type BoardTile = Tile;

/// Lifecycle state represented in snapshots and results.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GamePhase {
    /// Moves are accepted.
    Active,
    /// Final result is immutable.
    Finished,
}

/// One main or cross word validated and scored atomically.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FormedWord {
    /// Canonical A-Z board spelling, including blank assignments.
    pub text: String,
    /// Exact key queried in the pinned pack.
    pub normalized: String,
    /// Ordered squares occupied by the word.
    pub coordinates: Vec<Coordinate>,
    /// Letter score. Blank tiles contribute zero.
    pub score: u32,
}

/// Public deterministic game state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicGameState {
    /// Stable caller-supplied game ID.
    pub game_id: String,
    /// Immutable ruleset ID.
    pub ruleset_id: RulesetId,
    /// Exact pack selected during creation.
    pub lexicon: PackIdentity,
    /// Row-major 15x15 public board.
    pub board: Vec<Option<BoardTile>>,
    /// Scores for players one and two.
    pub scores: [u32; 2],
    /// Seat allowed to play next.
    pub current_player: Player,
    /// Number of committed post-creation mutations.
    pub version: u64,
    /// Active or finished lifecycle.
    pub phase: GamePhase,
}

impl PublicGameState {
    /// Returns one occupied square when the coordinate is in bounds.
    #[must_use]
    pub fn tile_at(&self, coordinate: Coordinate) -> Option<&BoardTile> {
        coordinate
            .in_bounds(BOARD_SIZE, BOARD_SIZE)
            .then(|| {
                self.board
                    .get(coordinate.index(BOARD_SIZE))
                    .and_then(Option::as_ref)
            })
            .flatten()
    }
}

/// Persistable state checkpoint with the exact pack identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameSnapshot {
    /// Snapshot schema.
    pub schema_version: u32,
    /// Complete public state.
    pub state: PublicGameState,
}

/// Final game summary with immutable reproducibility input.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameResult {
    /// Game ID.
    pub game_id: String,
    /// Ruleset used by the game.
    pub ruleset_id: RulesetId,
    /// Exact lexicon used by every move.
    pub lexicon: PackIdentity,
    /// Final scores.
    pub scores: [u32; 2],
    /// Winning seat, or `None` for a tie.
    pub winner: Option<Player>,
    /// Final state version.
    pub final_version: u64,
}

/// Event payload emitted only after an atomic transition succeeds.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GameEventKind {
    /// Creation binds rules and lexicon before any move.
    Created {
        /// New game ID.
        game_id: String,
        /// Full versioned ruleset.
        ruleset: Ruleset,
    },
    /// One legal placement and every word it formed.
    MovePlayed {
        /// Acting seat.
        player: Player,
        /// Canonically ordered new tiles.
        placements: Vec<Placement>,
        /// Main word followed by cross words in board order.
        words: Vec<FormedWord>,
        /// Sum of all formed-word scores.
        score: u32,
        /// Scores after commit.
        scores_after: [u32; 2],
        /// Next active seat.
        next_player: Player,
    },
    /// Explicit immutable completion.
    Finished {
        /// Final result, including the exact pack identity.
        result: GameResult,
    },
}

/// Ordered immutable game event. Every event repeats the pack identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameEvent {
    /// Zero-based creation event, then state versions for mutations.
    pub sequence: u64,
    /// Exact lexicon active for this event.
    pub lexicon: PackIdentity,
    /// Event-specific data.
    pub kind: GameEventKind,
}

/// Portable replay input containing all deterministic lexicon bindings.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayBundle {
    /// Replay schema.
    pub schema_version: u32,
    /// Full versioned ruleset.
    pub ruleset: Ruleset,
    /// Exact pack required to replay.
    pub lexicon: PackIdentity,
    /// Complete ordered event stream.
    pub events: Vec<GameEvent>,
}

/// Active deterministic game and its immutable lookup instance.
#[derive(Debug)]
pub struct Game {
    ruleset: Ruleset,
    lexicon: Arc<dyn WordValidator>,
    state: PublicGameState,
    events: Vec<GameEvent>,
}

impl Game {
    /// Creates a game only after the exact ruleset pack is available.
    ///
    /// # Errors
    ///
    /// Returns [`GameError::MissingLexicon`] or an exact-identity mismatch
    /// before creating state or emitting an event.
    pub fn create(
        game_id: impl Into<String>,
        ruleset: Ruleset,
        lexicon: Option<Arc<dyn WordValidator>>,
    ) -> Result<Self, GameError> {
        ruleset.validate()?;
        let lexicon = lexicon.ok_or_else(|| GameError::MissingLexicon {
            ruleset: ruleset.id,
            required: Box::new(ruleset.lexicon.clone()),
        })?;
        ruleset.ensure_lexicon(CompatibilityContext::Ruleset, lexicon.identity())?;
        let game_id = game_id.into();
        let identity = lexicon.identity().clone();
        let state = PublicGameState {
            game_id: game_id.clone(),
            ruleset_id: ruleset.id,
            lexicon: identity.clone(),
            board: vec![None; BOARD_SQUARES],
            scores: [0, 0],
            current_player: Player::One,
            version: 0,
            phase: GamePhase::Active,
        };
        let event = GameEvent {
            sequence: 0,
            lexicon: identity,
            kind: GameEventKind::Created {
                game_id,
                ruleset: ruleset.clone(),
            },
        };
        Ok(Self {
            ruleset,
            lexicon,
            state,
            events: vec![event],
        })
    }

    /// Restores an exact snapshot without selecting another installed pack.
    ///
    /// # Errors
    ///
    /// Returns before producing a resumable game when schema, board, ruleset,
    /// or complete pack identity differs.
    pub fn resume(
        snapshot: GameSnapshot,
        ruleset: Ruleset,
        lexicon: Option<Arc<dyn WordValidator>>,
    ) -> Result<Self, GameError> {
        if snapshot.schema_version != SNAPSHOT_SCHEMA_VERSION {
            return Err(GameError::UnsupportedSchema {
                artifact: "snapshot",
                found: snapshot.schema_version,
                expected: SNAPSHOT_SCHEMA_VERSION,
            });
        }
        ruleset.validate()?;
        if snapshot.state.ruleset_id != ruleset.id {
            return Err(GameError::RulesetMismatch {
                expected: snapshot.state.ruleset_id,
                actual: ruleset.id,
            });
        }
        if snapshot.state.board.len() != BOARD_SQUARES {
            return Err(GameError::InvalidSnapshotBoard {
                actual: snapshot.state.board.len(),
                expected: BOARD_SQUARES,
            });
        }
        let lexicon = lexicon.ok_or_else(|| GameError::MissingLexicon {
            ruleset: ruleset.id,
            required: Box::new(snapshot.state.lexicon.clone()),
        })?;
        ruleset.ensure_lexicon(CompatibilityContext::ActiveGame, lexicon.identity())?;
        word_arena_lexicon::ensure_exact_pack(
            CompatibilityContext::ActiveGame,
            &snapshot.state.lexicon,
            lexicon.identity(),
        )?;
        for tile in snapshot.state.board.iter().flatten() {
            let canonical =
                canonical_tile_token(&ruleset.lexicon.normalization.profile, &tile.letter)?;
            if canonical != tile.letter {
                return Err(GameError::NonCanonicalBoardTile {
                    token: tile.letter.clone(),
                    canonical,
                });
            }
        }
        Ok(Self {
            ruleset,
            lexicon,
            state: snapshot.state,
            events: Vec::new(),
        })
    }

    /// Validates all main/cross words and scores before committing a placement.
    ///
    /// # Errors
    ///
    /// Returns a placement or lexicon error without changing public state,
    /// scores, turn, version, board, or events.
    pub fn play_tiles(
        &mut self,
        player: Player,
        placements: Vec<Placement>,
    ) -> Result<GameEvent, GameError> {
        if self.state.phase == GamePhase::Finished {
            return Err(GameError::GameFinished);
        }
        if player != self.state.current_player {
            return Err(GameError::WrongPlayer {
                expected: self.state.current_player,
                actual: player,
            });
        }
        let prepared = self.prepare_placement(placements)?;
        let updated_score = self.state.scores[player.index()]
            .checked_add(prepared.score)
            .ok_or(GameError::ScoreOverflow)?;
        let updated_version = self
            .state
            .version
            .checked_add(1)
            .ok_or(GameError::VersionOverflow)?;
        for placement in &prepared.placements {
            self.state.board[placement.coordinate.index(BOARD_SIZE)] = Some(placement.tile.clone());
        }
        self.state.scores[player.index()] = updated_score;
        self.state.current_player = player.opponent();
        self.state.version = updated_version;
        let event = GameEvent {
            sequence: self.state.version,
            lexicon: self.state.lexicon.clone(),
            kind: GameEventKind::MovePlayed {
                player,
                placements: prepared.placements,
                words: prepared.words,
                score: prepared.score,
                scores_after: self.state.scores,
                next_player: self.state.current_player,
            },
        };
        self.events.push(event.clone());
        Ok(event)
    }

    /// Explicitly finalizes the game and records an immutable result event.
    ///
    /// # Errors
    ///
    /// Returns [`GameError::GameFinished`] when already finalized.
    pub fn finish(&mut self) -> Result<GameResult, GameError> {
        if self.state.phase == GamePhase::Finished {
            return Err(GameError::GameFinished);
        }
        let updated_version = self
            .state
            .version
            .checked_add(1)
            .ok_or(GameError::VersionOverflow)?;
        self.state.phase = GamePhase::Finished;
        self.state.version = updated_version;
        let result = self.current_result();
        self.events.push(GameEvent {
            sequence: self.state.version,
            lexicon: self.state.lexicon.clone(),
            kind: GameEventKind::Finished {
                result: result.clone(),
            },
        });
        Ok(result)
    }

    /// Replays and revalidates every event using the exact recorded pack.
    ///
    /// # Errors
    ///
    /// Rejects absent/substituted packs before game creation and rejects any
    /// event that differs from deterministic recomputation.
    pub fn replay(
        bundle: &ReplayBundle,
        lexicon: Option<Arc<dyn WordValidator>>,
    ) -> Result<Self, GameError> {
        if bundle.schema_version != REPLAY_SCHEMA_VERSION {
            return Err(GameError::UnsupportedSchema {
                artifact: "replay",
                found: bundle.schema_version,
                expected: REPLAY_SCHEMA_VERSION,
            });
        }
        bundle.ruleset.validate()?;
        let lexicon = lexicon.ok_or_else(|| GameError::MissingLexicon {
            ruleset: bundle.ruleset.id,
            required: Box::new(bundle.lexicon.clone()),
        })?;
        bundle
            .ruleset
            .ensure_lexicon(CompatibilityContext::Replay, lexicon.identity())?;
        word_arena_lexicon::ensure_exact_pack(
            CompatibilityContext::Replay,
            &bundle.lexicon,
            lexicon.identity(),
        )?;
        let Some(created) = bundle.events.first() else {
            return Err(GameError::InvalidReplayEvent {
                sequence: 0,
                reason: "creation event is required",
            });
        };
        let GameEventKind::Created { game_id, ruleset } = &created.kind else {
            return Err(GameError::InvalidReplayEvent {
                sequence: created.sequence,
                reason: "first event must create the game",
            });
        };
        if created.sequence != 0 || ruleset != &bundle.ruleset || created.lexicon != bundle.lexicon
        {
            return Err(GameError::ReplayEventMismatch {
                sequence: created.sequence,
            });
        }
        let mut game = Self::create(game_id.clone(), bundle.ruleset.clone(), Some(lexicon))?;
        if game.events[0] != *created {
            return Err(GameError::ReplayEventMismatch { sequence: 0 });
        }
        for expected in &bundle.events[1..] {
            if expected.lexicon != bundle.lexicon {
                return Err(GameError::ReplayEventMismatch {
                    sequence: expected.sequence,
                });
            }
            match &expected.kind {
                GameEventKind::Created { .. } => {
                    return Err(GameError::InvalidReplayEvent {
                        sequence: expected.sequence,
                        reason: "creation may occur only once",
                    });
                }
                GameEventKind::MovePlayed {
                    player, placements, ..
                } => {
                    game.play_tiles(*player, placements.clone())?;
                }
                GameEventKind::Finished { .. } => {
                    game.finish()?;
                }
            }
            if game.events.last() != Some(expected) {
                return Err(GameError::ReplayEventMismatch {
                    sequence: expected.sequence,
                });
            }
        }
        Ok(game)
    }

    /// Current immutable public state.
    #[must_use]
    pub const fn public_state(&self) -> &PublicGameState {
        &self.state
    }

    /// Events emitted by this instance. A resumed instance starts a new tail.
    #[must_use]
    pub fn events(&self) -> &[GameEvent] {
        &self.events
    }

    /// Creates a persistable state checkpoint.
    #[must_use]
    pub fn snapshot(&self) -> GameSnapshot {
        GameSnapshot {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            state: self.state.clone(),
        }
    }

    /// Creates a portable replay bundle when this instance has creation history.
    ///
    /// A snapshot-only resumed instance returns `None`; persistence must combine
    /// its stored creation/history events before publishing a replay.
    #[must_use]
    pub fn replay_bundle(&self) -> Option<ReplayBundle> {
        matches!(
            self.events.first().map(|event| &event.kind),
            Some(GameEventKind::Created { .. })
        )
        .then(|| ReplayBundle {
            schema_version: REPLAY_SCHEMA_VERSION,
            ruleset: self.ruleset.clone(),
            lexicon: self.state.lexicon.clone(),
            events: self.events.clone(),
        })
    }

    /// Returns a result only after explicit completion.
    #[must_use]
    pub fn result(&self) -> Option<GameResult> {
        (self.state.phase == GamePhase::Finished).then(|| self.current_result())
    }

    fn current_result(&self) -> GameResult {
        let winner = match self.state.scores[0].cmp(&self.state.scores[1]) {
            std::cmp::Ordering::Greater => Some(Player::One),
            std::cmp::Ordering::Less => Some(Player::Two),
            std::cmp::Ordering::Equal => None,
        };
        GameResult {
            game_id: self.state.game_id.clone(),
            ruleset_id: self.state.ruleset_id,
            lexicon: self.state.lexicon.clone(),
            scores: self.state.scores,
            winner,
            final_version: self.state.version,
        }
    }

    fn prepare_placement(&self, mut placements: Vec<Placement>) -> Result<PreparedMove, GameError> {
        if placements.is_empty() {
            return Err(GameError::EmptyPlacement);
        }
        for placement in &mut placements {
            if !placement.coordinate.in_bounds(BOARD_SIZE, BOARD_SIZE) {
                return Err(GameError::CoordinateOutOfBounds {
                    coordinate: placement.coordinate,
                });
            }
            placement.tile.letter = canonical_tile_token(
                &self.ruleset.lexicon.normalization.profile,
                &placement.tile.letter,
            )?;
            if self.state.tile_at(placement.coordinate).is_some() {
                return Err(GameError::OccupiedSquare {
                    coordinate: placement.coordinate,
                });
            }
        }
        let unique = placements
            .iter()
            .map(|placement| placement.coordinate)
            .collect::<BTreeSet<_>>();
        if unique.len() != placements.len() {
            let duplicate = placements
                .iter()
                .map(|placement| placement.coordinate)
                .find(|coordinate| {
                    placements
                        .iter()
                        .filter(|placement| placement.coordinate == *coordinate)
                        .count()
                        > 1
                })
                .expect("a duplicate coordinate exists");
            return Err(GameError::DuplicateCoordinate {
                coordinate: duplicate,
            });
        }
        let orientation = orientation(&placements)?;
        placements.sort_unstable_by_key(|placement| match orientation {
            Orientation::Horizontal => (placement.coordinate.column, placement.coordinate.row),
            Orientation::Vertical | Orientation::Single => {
                (placement.coordinate.row, placement.coordinate.column)
            }
        });
        let proposed = placements
            .iter()
            .map(|placement| (placement.coordinate, &placement.tile))
            .collect::<BTreeMap<_, _>>();
        self.validate_contiguous(&placements, orientation, &proposed)?;
        self.validate_connection(&placements, &proposed)?;
        let word_coordinates = self.formed_word_coordinates(&placements, orientation, &proposed);
        if word_coordinates.is_empty() {
            return Err(GameError::NoWordFormed);
        }
        let mut words = Vec::with_capacity(word_coordinates.len());
        let mut score = 0_u32;
        for coordinates in word_coordinates {
            let word = self.validate_word(coordinates, &proposed)?;
            score = score
                .checked_add(word.score)
                .ok_or(GameError::ScoreOverflow)?;
            words.push(word);
        }
        Ok(PreparedMove {
            placements,
            words,
            score,
        })
    }

    fn validate_contiguous(
        &self,
        placements: &[Placement],
        orientation: Orientation,
        proposed: &BTreeMap<Coordinate, &Tile>,
    ) -> Result<(), GameError> {
        if orientation == Orientation::Single {
            return Ok(());
        }
        let first = placements
            .first()
            .expect("placement is nonempty")
            .coordinate;
        let last = placements.last().expect("placement is nonempty").coordinate;
        match orientation {
            Orientation::Horizontal => {
                for column in first.column..=last.column {
                    let coordinate = Coordinate::new(first.row, column);
                    if self.tile_with_proposed(coordinate, proposed).is_none() {
                        return Err(GameError::NotContiguous { coordinate });
                    }
                }
            }
            Orientation::Vertical => {
                for row in first.row..=last.row {
                    let coordinate = Coordinate::new(row, first.column);
                    if self.tile_with_proposed(coordinate, proposed).is_none() {
                        return Err(GameError::NotContiguous { coordinate });
                    }
                }
            }
            Orientation::Single => {}
        }
        Ok(())
    }

    fn validate_connection(
        &self,
        placements: &[Placement],
        proposed: &BTreeMap<Coordinate, &Tile>,
    ) -> Result<(), GameError> {
        let board_is_empty = self.state.board.iter().all(Option::is_none);
        if board_is_empty {
            if proposed.contains_key(&CENTER) {
                return Ok(());
            }
            return Err(GameError::OpeningMoveMissesCenter);
        }
        if placements.iter().any(|placement| {
            neighbors(placement.coordinate)
                .into_iter()
                .flatten()
                .any(|coordinate| self.state.tile_at(coordinate).is_some())
        }) {
            Ok(())
        } else {
            Err(GameError::DisconnectedPlacement)
        }
    }

    fn formed_word_coordinates(
        &self,
        placements: &[Placement],
        orientation: Orientation,
        proposed: &BTreeMap<Coordinate, &Tile>,
    ) -> Vec<Vec<Coordinate>> {
        let mut words = Vec::new();
        match orientation {
            Orientation::Horizontal => {
                push_if_word(
                    &mut words,
                    self.word_segment(placements[0].coordinate, 0, 1, proposed),
                );
                for placement in placements {
                    push_if_word(
                        &mut words,
                        self.word_segment(placement.coordinate, 1, 0, proposed),
                    );
                }
            }
            Orientation::Vertical => {
                push_if_word(
                    &mut words,
                    self.word_segment(placements[0].coordinate, 1, 0, proposed),
                );
                for placement in placements {
                    push_if_word(
                        &mut words,
                        self.word_segment(placement.coordinate, 0, 1, proposed),
                    );
                }
            }
            Orientation::Single => {
                push_if_word(
                    &mut words,
                    self.word_segment(placements[0].coordinate, 0, 1, proposed),
                );
                push_if_word(
                    &mut words,
                    self.word_segment(placements[0].coordinate, 1, 0, proposed),
                );
            }
        }
        let mut seen = BTreeSet::new();
        words.retain(|coordinates| seen.insert(coordinates.clone()));
        words
    }

    fn word_segment(
        &self,
        origin: Coordinate,
        row_step: i16,
        column_step: i16,
        proposed: &BTreeMap<Coordinate, &Tile>,
    ) -> Vec<Coordinate> {
        let mut row = i16::from(origin.row);
        let mut column = i16::from(origin.column);
        while let Some(previous) = coordinate_at(row - row_step, column - column_step) {
            if self.tile_with_proposed(previous, proposed).is_none() {
                break;
            }
            row -= row_step;
            column -= column_step;
        }
        let mut coordinates = Vec::new();
        while let Some(coordinate) = coordinate_at(row, column) {
            if self.tile_with_proposed(coordinate, proposed).is_none() {
                break;
            }
            coordinates.push(coordinate);
            row += row_step;
            column += column_step;
        }
        coordinates
    }

    fn validate_word(
        &self,
        coordinates: Vec<Coordinate>,
        proposed: &BTreeMap<Coordinate, &Tile>,
    ) -> Result<FormedWord, GameError> {
        let mut text = String::new();
        let mut score = 0_u32;
        for coordinate in &coordinates {
            let tile = self
                .tile_with_proposed(*coordinate, proposed)
                .expect("formed word coordinates are occupied");
            text.push_str(&tile.letter);
            if !tile.is_blank {
                let normalized =
                    normalize_key(&self.ruleset.lexicon.normalization.profile, &tile.letter)?;
                for letter in normalized.chars() {
                    score = score
                        .checked_add(self.ruleset.letter_score(letter))
                        .ok_or(GameError::ScoreOverflow)?;
                }
            }
        }
        let normalized = normalize_key(&self.ruleset.lexicon.normalization.profile, &text)?;
        if !self.lexicon.contains(&normalized) {
            return Err(GameError::InvalidWord {
                word: text,
                normalized: normalized.into_string(),
            });
        }
        Ok(FormedWord {
            text,
            normalized: normalized.into_string(),
            coordinates,
            score,
        })
    }

    fn tile_with_proposed<'a>(
        &'a self,
        coordinate: Coordinate,
        proposed: &'a BTreeMap<Coordinate, &Tile>,
    ) -> Option<&'a Tile> {
        proposed
            .get(&coordinate)
            .copied()
            .or_else(|| self.state.tile_at(coordinate))
    }
}

struct PreparedMove {
    placements: Vec<Placement>,
    words: Vec<FormedWord>,
    score: u32,
}

fn canonical_tile_token(profile: &str, token: &str) -> Result<String, GameError> {
    let normalized = normalize_key(profile, token)?.into_string();
    if normalized.len() == 1 {
        Ok(normalized)
    } else {
        Err(GameError::InvalidTileToken {
            token: token.to_owned(),
            normalized,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Orientation {
    Horizontal,
    Vertical,
    Single,
}

fn orientation(placements: &[Placement]) -> Result<Orientation, GameError> {
    if placements.len() == 1 {
        return Ok(Orientation::Single);
    }
    let same_row = placements
        .iter()
        .all(|placement| placement.coordinate.row == placements[0].coordinate.row);
    let same_column = placements
        .iter()
        .all(|placement| placement.coordinate.column == placements[0].coordinate.column);
    match (same_row, same_column) {
        (true, false) => Ok(Orientation::Horizontal),
        (false, true) => Ok(Orientation::Vertical),
        _ => Err(GameError::NotAligned),
    }
}

fn push_if_word(words: &mut Vec<Vec<Coordinate>>, coordinates: Vec<Coordinate>) {
    if coordinates.len() >= 2 {
        words.push(coordinates);
    }
}

fn coordinate_at(row: i16, column: i16) -> Option<Coordinate> {
    let row = u8::try_from(row).ok()?;
    let column = u8::try_from(column).ok()?;
    let coordinate = Coordinate::new(row, column);
    coordinate
        .in_bounds(BOARD_SIZE, BOARD_SIZE)
        .then_some(coordinate)
}

fn neighbors(coordinate: Coordinate) -> [Option<Coordinate>; 4] {
    [
        coordinate_at(i16::from(coordinate.row) - 1, i16::from(coordinate.column)),
        coordinate_at(i16::from(coordinate.row) + 1, i16::from(coordinate.column)),
        coordinate_at(i16::from(coordinate.row), i16::from(coordinate.column) - 1),
        coordinate_at(i16::from(coordinate.row), i16::from(coordinate.column) + 1),
    ]
}
