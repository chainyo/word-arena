use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use word_arena_lexicon::{CompatibilityContext, PackIdentity, normalize_key};

use crate::random::return_tiles_to_bag;
use crate::{
    Bag, Coordinate, GameError, GameSeed, PhysicalTile, Player, Premium, Rack, RngAlgorithm,
    Ruleset, RulesetId, RulesetIdentity, Score, Seat, SeedCommitment, TileFace, TileId, TileToken,
    WordValidator, prepare_initial_deal, verify_tile_conservation,
};

/// Width and height of the V1 board.
pub const BOARD_SIZE: u8 = 15;
const BOARD_SQUARES: usize = BOARD_SIZE as usize * BOARD_SIZE as usize;
const CENTER: Coordinate = Coordinate { row: 7, column: 7 };
const SNAPSHOT_SCHEMA_VERSION: u32 = 3;
const REPLAY_SCHEMA_VERSION: u32 = 3;
const PROJECTION_SCHEMA_VERSION: u32 = 1;

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
    /// Creates a regular scored tile assignment.
    #[must_use]
    pub fn letter(letter: impl Into<String>) -> Self {
        Self {
            letter: letter.into(),
            is_blank: false,
        }
    }

    /// Creates a zero-point blank assignment.
    #[must_use]
    pub fn blank(assigned_letter: impl Into<String>) -> Self {
        Self {
            letter: assigned_letter.into(),
            is_blank: true,
        }
    }
}

/// One owned physical tile and its target square.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    /// Stable tile identity from the acting rack.
    pub tile_id: TileId,
    /// Target square.
    pub coordinate: Coordinate,
    /// Board assignment, canonicalized before commit.
    pub tile: Tile,
}

impl Placement {
    /// Creates an owned placement value.
    #[must_use]
    pub const fn new(tile_id: TileId, coordinate: Coordinate, tile: Tile) -> Self {
        Self {
            tile_id,
            coordinate,
            tile,
        }
    }
}

/// Typed player action accepted by the complete game engine.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Move {
    /// Place one or more rack tiles on the board.
    Place {
        /// Proposed owned square assignments.
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

/// Immutable tile stored on the public board.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoardTile {
    /// Stable physical identity, public after placement.
    pub tile_id: TileId,
    /// Canonical A-Z board token, including a blank assignment.
    pub letter: String,
    /// Whether the physical tile has a blank face.
    pub is_blank: bool,
}

/// Lifecycle state represented in snapshots and results.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GamePhase {
    /// Moves are accepted.
    Active,
    /// Final result is immutable.
    Finished,
}

/// Immutable reason a game stopped accepting actions.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalReason {
    /// One seat conceded.
    Resignation {
        /// Seat that resigned.
        resigned: Seat,
    },
    /// The configured consecutive scoreless-turn limit was reached.
    ScorelessTurns,
    /// One seat emptied its rack after exhausting the bag.
    RackEmptied {
        /// Seat that went out.
        outgoing: Seat,
    },
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
    /// Letter subtotal after new-square letter premiums.
    pub letter_score: u32,
    /// Product of new-square word premiums.
    pub word_multiplier: u32,
    /// Checked `letter_score * word_multiplier` total.
    pub score: u32,
}

/// Public deterministic game state with no rack contents or future bag order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicGameState {
    /// Stable caller-supplied game ID.
    pub game_id: String,
    /// Immutable ruleset ID.
    pub ruleset_id: RulesetId,
    /// Immutable full ruleset content identity.
    pub ruleset: RulesetIdentity,
    /// Exact pack selected during creation.
    pub lexicon: PackIdentity,
    /// Versioned random contract used by this game.
    pub rng_algorithm: RngAlgorithm,
    /// Public seed commitment; the seed remains private during play.
    pub seed_commitment: SeedCommitment,
    /// Row-major 15x15 public board.
    pub board: Vec<Option<BoardTile>>,
    /// Scores for seats one and two.
    pub scores: [Score; 2],
    /// Seat allowed to play next.
    pub current_player: Player,
    /// Number of committed post-creation mutations.
    pub version: u64,
    /// Consecutive zero-score turns.
    pub scoreless_turns: u8,
    /// Number of private tiles owned by each rack.
    pub rack_counts: [u8; 2],
    /// Public number of tiles remaining in the private bag.
    pub bag_count: u16,
    /// Active or finished lifecycle.
    pub phase: GamePhase,
    /// Immutable completion data after a terminal transition.
    pub result: Option<GameResult>,
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

/// Authoritative persistable checkpoint. This is never a player projection.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameSnapshot {
    /// Snapshot schema.
    pub schema_version: u32,
    /// Full immutable ruleset content identity.
    pub ruleset: RulesetIdentity,
    /// Exact random algorithm used for setup and exchanges.
    pub rng_algorithm: RngAlgorithm,
    /// Complete public state.
    pub state: PublicGameState,
    /// Exact private bag order.
    pub bag: Bag,
    /// Exact private racks in seat order.
    pub racks: [Rack; 2],
    /// Private seed retained for resume and post-game replay reveal.
    pub seed: [u8; 32],
    /// Complete public event history through this checkpoint.
    pub events: Vec<GameEvent>,
    /// Complete deterministic private event history through this checkpoint.
    pub private_events: Vec<PrivateGameEvent>,
}

impl std::fmt::Debug for GameSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GameSnapshot")
            .field("schema_version", &self.schema_version)
            .field("ruleset", &self.ruleset)
            .field("rng_algorithm", &self.rng_algorithm)
            .field("state", &self.state)
            .field("bag", &"[REDACTED]")
            .field("racks", &"[REDACTED]")
            .field("seed", &"[REDACTED]")
            .field("public_event_count", &self.events.len())
            .field("private_event_count", &self.private_events.len())
            .finish()
    }
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
    pub scores: [Score; 2],
    /// Winning seat, or `None` for a tie.
    pub winner: Option<Player>,
    /// Final state version.
    pub final_version: u64,
    /// Exact terminal rule that ended the game.
    pub reason: TerminalReason,
}

/// Public event payload emitted only after an atomic transition succeeds.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GameEventKind {
    /// Creation binds every public reproducibility input except the live seed.
    Created {
        /// New game ID.
        game_id: String,
        /// Full versioned ruleset.
        ruleset: Ruleset,
        /// Independently versioned random algorithm.
        rng_algorithm: RngAlgorithm,
        /// Proof binding the eventual seed reveal.
        seed_commitment: SeedCommitment,
        /// Private rack sizes after the opening deal.
        rack_counts: [u8; 2],
        /// Remaining private bag size after the opening deal.
        bag_count: u16,
    },
    /// One legal placement and every word it formed.
    MovePlayed {
        /// Acting seat.
        player: Player,
        /// Canonically ordered new physical tiles.
        placements: Vec<Placement>,
        /// Main word followed by cross words in board order.
        words: Vec<FormedWord>,
        /// Bonus for using the configured full rack.
        bingo_bonus: u32,
        /// Sum of all formed words and the bingo bonus.
        score: u32,
        /// Number of private replacement tiles drawn.
        draw_count: u8,
        /// Public ownership counts after commit.
        rack_counts_after: [u8; 2],
        /// Remaining bag count after commit.
        bag_count_after: u16,
        /// Scores after commit.
        scores_after: [Score; 2],
        /// Scoreless counter after commit.
        scoreless_turns_after: u8,
        /// Next active seat.
        next_player: Player,
        /// Completion produced by this placement, if any.
        result: Option<GameResult>,
    },
    /// One scoreless pass.
    Passed {
        /// Acting seat.
        player: Player,
        /// Scoreless counter after commit.
        scoreless_turns_after: u8,
        /// Next seat when still active.
        next_player: Player,
        /// Completion produced by this pass, if any.
        result: Option<GameResult>,
    },
    /// One deterministic tile exchange.
    Exchanged {
        /// Acting seat.
        player: Player,
        /// Canonically ordered returned physical IDs.
        tile_ids: Vec<TileId>,
        /// Public ownership counts after commit.
        rack_counts_after: [u8; 2],
        /// Bag count after commit.
        bag_count_after: u16,
        /// Scoreless counter after commit.
        scoreless_turns_after: u8,
        /// Next seat when still active.
        next_player: Player,
        /// Completion produced by this exchange, if any.
        result: Option<GameResult>,
    },
    /// One seat conceded and ended the game immediately.
    Resigned {
        /// Resigning seat.
        player: Player,
        /// Immutable terminal result.
        result: GameResult,
    },
}

/// Ordered immutable public game event.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "scope", content = "seat", rename_all = "snake_case")]
pub enum EventVisibility {
    /// Safe for every game observer.
    Public,
    /// Visible only to the named seat or trusted human/operator roles.
    SeatPrivate(Seat),
}

/// Ordered immutable public game event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameEvent {
    /// Zero-based creation event, then state versions for mutations.
    pub sequence: u64,
    /// Visibility fixed when the event is created.
    pub visibility: EventVisibility,
    /// Exact lexicon active for this event.
    pub lexicon: PackIdentity,
    /// Event-specific data.
    pub kind: GameEventKind,
}

/// Seat-private tile transition paired with one public move event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateGameEvent {
    /// Matching public event sequence.
    pub sequence: u64,
    /// Seat-private visibility fixed when the event is created.
    pub visibility: EventVisibility,
    /// Only this seat may receive the projection during live play.
    pub seat: Seat,
    /// Exact owned physical tiles removed by the move.
    pub removed: Vec<PhysicalTile>,
    /// Exact replacement tiles received from the bag.
    pub drawn: Vec<PhysicalTile>,
    /// Acting seat's complete rack after the transition.
    pub rack_after: Rack,
}

/// Portable post-game replay input, including an explicit seed reveal.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayBundle {
    /// Replay schema.
    pub schema_version: u32,
    /// Full immutable ruleset content identity.
    pub ruleset_identity: RulesetIdentity,
    /// Full versioned ruleset.
    pub ruleset: Ruleset,
    /// Exact pack required to replay.
    pub lexicon: PackIdentity,
    /// Exact random algorithm bound by creation and seed commitment.
    pub rng_algorithm: RngAlgorithm,
    /// Post-game seed reveal.
    pub seed_reveal: [u8; 32],
    /// Complete ordered public event stream.
    pub events: Vec<GameEvent>,
    /// Complete ordered seat-private placement transitions.
    pub private_events: Vec<PrivateGameEvent>,
}

/// Public role projection with no rack, seed, private draw, or bag-order data.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicProjection {
    /// Projection schema.
    pub schema_version: u32,
    /// Current public state.
    pub state: PublicGameState,
    /// Complete public event history.
    pub events: Vec<GameEvent>,
}

/// One authenticated competitive seat's private projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeatProjection {
    /// Projection schema.
    pub schema_version: u32,
    /// Seat this projection belongs to.
    pub seat: Seat,
    /// Shared public state and history.
    pub public: PublicProjection,
    /// Only the selected seat's current rack.
    pub rack: Rack,
    /// Only the selected seat's private transition history.
    pub private_events: Vec<PrivateGameEvent>,
}

/// Human-only live spectator projection, deliberately distinct from a seat.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HumanSpectatorProjection {
    /// Projection schema.
    pub schema_version: u32,
    /// Shared public state and history.
    pub public: PublicProjection,
    /// Both current racks in stable seat order.
    pub racks: [Rack; 2],
    /// Past private transitions for both seats, never the future bag.
    pub private_events: Vec<PrivateGameEvent>,
}

/// Trusted operator projection with the complete authoritative checkpoint.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdministratorProjection {
    /// Projection schema.
    pub schema_version: u32,
    /// Complete durable authoritative game data.
    pub snapshot: GameSnapshot,
}

/// Active deterministic game and its immutable lookup instance.
pub struct Game {
    ruleset: Ruleset,
    lexicon: Arc<dyn WordValidator>,
    seed: GameSeed,
    bag: Bag,
    racks: [Rack; 2],
    state: PublicGameState,
    events: Vec<GameEvent>,
    private_events: Vec<PrivateGameEvent>,
}

impl std::fmt::Debug for Game {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Game")
            .field("ruleset", &self.ruleset.id)
            .field("state", &self.state)
            .field("seed", &self.seed)
            .field("bag", &"[REDACTED]")
            .field("racks", &"[REDACTED]")
            .field("public_event_count", &self.events.len())
            .field("private_event_count", &self.private_events.len())
            .finish_non_exhaustive()
    }
}

impl Game {
    /// Creates, shuffles, and deals a game from an application-supplied seed.
    ///
    /// # Errors
    ///
    /// Returns before creating state when the ruleset, exact lexicon, seed
    /// setup, count conversion, or conservation contract fails.
    pub fn create(
        game_id: impl Into<String>,
        ruleset: Ruleset,
        lexicon: Option<Arc<dyn WordValidator>>,
        seed: GameSeed,
    ) -> Result<Self, GameError> {
        ruleset.validate()?;
        let lexicon = lexicon.ok_or_else(|| GameError::MissingLexicon {
            ruleset: ruleset.id,
            required: Box::new(ruleset.lexicon.clone()),
        })?;
        ruleset.ensure_lexicon(CompatibilityContext::Ruleset, lexicon.identity())?;
        let deal = prepare_initial_deal(&ruleset, &seed).map_err(tile_state_error)?;
        let algorithm = deal.algorithm();
        let commitment = deal.commitment().clone();
        let (bag, racks) = deal.into_parts();
        let rack_counts = rack_counts(&racks)?;
        let bag_count = count_u16(bag.len())?;
        let game_id = game_id.into();
        let identity = lexicon.identity().clone();
        let state = PublicGameState {
            game_id: game_id.clone(),
            ruleset_id: ruleset.id,
            ruleset: ruleset.identity(),
            lexicon: identity.clone(),
            rng_algorithm: algorithm,
            seed_commitment: commitment.clone(),
            board: vec![None; BOARD_SQUARES],
            scores: [Score::ZERO, Score::ZERO],
            current_player: Player::One,
            version: 0,
            scoreless_turns: 0,
            rack_counts,
            bag_count,
            phase: GamePhase::Active,
            result: None,
        };
        let event = GameEvent {
            sequence: 0,
            visibility: EventVisibility::Public,
            lexicon: identity,
            kind: GameEventKind::Created {
                game_id,
                ruleset: ruleset.clone(),
                rng_algorithm: algorithm,
                seed_commitment: commitment,
                rack_counts,
                bag_count,
            },
        };
        Ok(Self {
            ruleset,
            lexicon,
            seed,
            bag,
            racks,
            state,
            events: vec![event],
            private_events: Vec::new(),
        })
    }

    /// Restores an authoritative checkpoint with exact private ownership.
    ///
    /// # Errors
    ///
    /// Returns before producing a resumable game when schema, ruleset, pack,
    /// seed commitment, board, public counts, or conservation differs.
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
        let ruleset_identity = ruleset.identity();
        if snapshot.state.ruleset_id != ruleset.id
            || snapshot.state.ruleset != ruleset_identity
            || snapshot.ruleset != ruleset_identity
        {
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
        if snapshot.rng_algorithm != snapshot.state.rng_algorithm
            || snapshot.rng_algorithm != snapshot.state.seed_commitment.algorithm
        {
            return Err(GameError::InvalidTileState {
                reason: "snapshot RNG identities differ".to_owned(),
            });
        }
        let bundle = ReplayBundle {
            schema_version: REPLAY_SCHEMA_VERSION,
            ruleset_identity,
            ruleset,
            lexicon: snapshot.state.lexicon.clone(),
            rng_algorithm: snapshot.rng_algorithm,
            seed_reveal: snapshot.seed,
            events: snapshot.events.clone(),
            private_events: snapshot.private_events.clone(),
        };
        let replayed = Self::replay(&bundle, lexicon)?;
        if replayed.state != snapshot.state
            || replayed.bag != snapshot.bag
            || replayed.racks != snapshot.racks
            || replayed.events != snapshot.events
            || replayed.private_events != snapshot.private_events
        {
            return Err(GameError::InvalidTileState {
                reason: "snapshot state differs from deterministic event replay".to_owned(),
            });
        }
        drop(snapshot);
        Ok(replayed)
    }

    /// Validates and commits one complete placement/refill transaction.
    ///
    /// # Errors
    ///
    /// Every validation, scoring, ownership, overflow, refill, and conservation
    /// failure returns without changing any authoritative or event state.
    pub fn play_tiles(
        &mut self,
        player: Player,
        expected_version: u64,
        placements: Vec<Placement>,
    ) -> Result<GameEvent, GameError> {
        self.validate_action(player, expected_version)?;
        let prepared = self.prepare_placement(player, placements)?;
        let score_delta = i32::try_from(prepared.score).map_err(|_| GameError::ScoreOverflow)?;
        let updated_score = self.state.scores[player.index()]
            .checked_add(score_delta)
            .ok_or(GameError::ScoreOverflow)?;
        let updated_version = self
            .state
            .version
            .checked_add(1)
            .ok_or(GameError::VersionOverflow)?;
        let scoreless_turns = next_scoreless(self.state.scoreless_turns, prepared.score)?;

        let played_ids = prepared
            .placements
            .iter()
            .map(|placement| placement.tile_id)
            .collect::<BTreeSet<_>>();
        let mut next_racks = self.racks.clone();
        let retained = next_racks[player.index()]
            .tiles()
            .iter()
            .filter(|tile| !played_ids.contains(&tile.id))
            .cloned()
            .collect();
        next_racks[player.index()] = Rack::new(retained);
        let mut next_bag = self.bag.clone();
        let refill_count = usize::from(self.ruleset.game.rack_capacity)
            .checked_sub(next_racks[player.index()].len())
            .ok_or_else(|| GameError::InvalidTileState {
                reason: "rack exceeds configured capacity".to_owned(),
            })?;
        let drawn = next_bag.draw_up_to(refill_count);
        next_racks[player.index()].extend(drawn.iter().cloned());

        let mut next_state = self.state.clone();
        for placement in &prepared.placements {
            next_state.board[placement.coordinate.index(BOARD_SIZE)] = Some(BoardTile {
                tile_id: placement.tile_id,
                letter: placement.tile.letter.clone(),
                is_blank: placement.tile.is_blank,
            });
        }
        next_state.scores[player.index()] = updated_score;
        next_state.current_player = player.opponent();
        next_state.version = updated_version;
        next_state.scoreless_turns = scoreless_turns;
        next_state.rack_counts = rack_counts(&next_racks)?;
        next_state.bag_count = count_u16(next_bag.len())?;

        let board = physical_board(&next_state, &self.ruleset)?;
        verify_tile_conservation(&self.ruleset, &next_bag, &next_racks, &board)
            .map_err(tile_state_error)?;
        let result = if next_bag.is_empty() && next_racks[player.index()].is_empty() {
            Some(self.complete_state(
                &mut next_state,
                &next_racks,
                TerminalReason::RackEmptied { outgoing: player },
            )?)
        } else if next_state.scoreless_turns >= self.ruleset.game.scoreless_turn_limit {
            Some(self.complete_state(
                &mut next_state,
                &next_racks,
                TerminalReason::ScorelessTurns,
            )?)
        } else {
            None
        };

        let event = GameEvent {
            sequence: updated_version,
            visibility: EventVisibility::Public,
            lexicon: next_state.lexicon.clone(),
            kind: GameEventKind::MovePlayed {
                player,
                placements: prepared.placements,
                words: prepared.words,
                bingo_bonus: prepared.bingo_bonus,
                score: prepared.score,
                draw_count: count_u8(drawn.len())?,
                rack_counts_after: next_state.rack_counts,
                bag_count_after: next_state.bag_count,
                scores_after: next_state.scores,
                scoreless_turns_after: next_state.scoreless_turns,
                next_player: next_state.current_player,
                result,
            },
        };
        let private_event = PrivateGameEvent {
            sequence: updated_version,
            visibility: EventVisibility::SeatPrivate(player),
            seat: player,
            removed: prepared.played,
            drawn,
            rack_after: next_racks[player.index()].clone(),
        };

        self.bag = next_bag;
        self.racks = next_racks;
        self.state = next_state;
        self.events.push(event.clone());
        self.private_events.push(private_event);
        Ok(event)
    }

    /// Applies one typed action through the same atomic transition methods.
    ///
    /// # Errors
    ///
    /// Returns the action-specific deterministic validation error without
    /// mutation.
    pub fn apply_move(
        &mut self,
        player: Player,
        expected_version: u64,
        action: Move,
    ) -> Result<GameEvent, GameError> {
        match action {
            Move::Place { placements } => self.play_tiles(player, expected_version, placements),
            Move::Exchange { tile_ids } => self.exchange_tiles(player, expected_version, tile_ids),
            Move::Pass => self.pass(player, expected_version),
            Move::Resign => self.resign(player, expected_version),
        }
    }

    /// Commits one scoreless pass and advances or completes the game.
    ///
    /// # Errors
    ///
    /// Wrong-seat, stale, finished, counter, version, or end-score failures are
    /// returned without mutation.
    pub fn pass(&mut self, player: Player, expected_version: u64) -> Result<GameEvent, GameError> {
        self.validate_action(player, expected_version)?;
        let updated_version = self
            .state
            .version
            .checked_add(1)
            .ok_or(GameError::VersionOverflow)?;
        let mut next_state = self.state.clone();
        next_state.version = updated_version;
        next_state.current_player = player.opponent();
        next_state.scoreless_turns = next_state
            .scoreless_turns
            .checked_add(1)
            .ok_or(GameError::ScorelessTurnOverflow)?;
        let result = if next_state.scoreless_turns >= self.ruleset.game.scoreless_turn_limit {
            Some(self.complete_state(
                &mut next_state,
                &self.racks,
                TerminalReason::ScorelessTurns,
            )?)
        } else {
            None
        };
        let event = GameEvent {
            sequence: updated_version,
            visibility: EventVisibility::Public,
            lexicon: next_state.lexicon.clone(),
            kind: GameEventKind::Passed {
                player,
                scoreless_turns_after: next_state.scoreless_turns,
                next_player: next_state.current_player,
                result,
            },
        };
        self.state = next_state;
        self.events.push(event.clone());
        Ok(event)
    }

    /// Exchanges owned tiles through the versioned deterministic bag policy.
    ///
    /// # Errors
    ///
    /// Empty, duplicate, unowned, undersized-bag, stale, overflow, or
    /// conservation failures leave every authoritative field unchanged.
    pub fn exchange_tiles(
        &mut self,
        player: Player,
        expected_version: u64,
        mut tile_ids: Vec<TileId>,
    ) -> Result<GameEvent, GameError> {
        self.validate_action(player, expected_version)?;
        if tile_ids.is_empty() {
            return Err(GameError::EmptyExchange);
        }
        tile_ids.sort_unstable();
        if let Some(duplicate) = tile_ids.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(GameError::DuplicatePlacementTile {
                tile_id: duplicate[0],
            });
        }
        let bag_count = count_u16(self.bag.len())?;
        if bag_count < self.ruleset.game.exchange_minimum {
            return Err(GameError::ExchangeBagTooSmall {
                required: self.ruleset.game.exchange_minimum,
                actual: bag_count,
            });
        }
        let rack = &self.racks[player.index()];
        let returned = tile_ids
            .iter()
            .map(|tile_id| {
                rack.tiles()
                    .iter()
                    .find(|tile| tile.id == *tile_id)
                    .cloned()
                    .ok_or(GameError::TileNotOwned { tile_id: *tile_id })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let updated_version = self
            .state
            .version
            .checked_add(1)
            .ok_or(GameError::VersionOverflow)?;
        let selected = tile_ids.iter().copied().collect::<BTreeSet<_>>();
        let retained = rack
            .tiles()
            .iter()
            .filter(|tile| !selected.contains(&tile.id))
            .cloned()
            .collect();
        let mut next_racks = self.racks.clone();
        next_racks[player.index()] = Rack::new(retained);
        let mut next_bag = self.bag.clone();
        let drawn = next_bag.draw_up_to(returned.len());
        next_racks[player.index()].extend(drawn.iter().cloned());
        return_tiles_to_bag(&mut next_bag, returned.clone(), &self.seed, updated_version);

        let mut next_state = self.state.clone();
        next_state.version = updated_version;
        next_state.current_player = player.opponent();
        next_state.scoreless_turns = next_state
            .scoreless_turns
            .checked_add(1)
            .ok_or(GameError::ScorelessTurnOverflow)?;
        next_state.rack_counts = rack_counts(&next_racks)?;
        next_state.bag_count = count_u16(next_bag.len())?;
        let board = physical_board(&next_state, &self.ruleset)?;
        verify_tile_conservation(&self.ruleset, &next_bag, &next_racks, &board)
            .map_err(tile_state_error)?;
        let result = if next_state.scoreless_turns >= self.ruleset.game.scoreless_turn_limit {
            Some(self.complete_state(
                &mut next_state,
                &next_racks,
                TerminalReason::ScorelessTurns,
            )?)
        } else {
            None
        };
        let event = GameEvent {
            sequence: updated_version,
            visibility: EventVisibility::Public,
            lexicon: next_state.lexicon.clone(),
            kind: GameEventKind::Exchanged {
                player,
                tile_ids,
                rack_counts_after: next_state.rack_counts,
                bag_count_after: next_state.bag_count,
                scoreless_turns_after: next_state.scoreless_turns,
                next_player: next_state.current_player,
                result,
            },
        };
        let private_event = PrivateGameEvent {
            sequence: updated_version,
            visibility: EventVisibility::SeatPrivate(player),
            seat: player,
            removed: returned,
            drawn,
            rack_after: next_racks[player.index()].clone(),
        };
        self.bag = next_bag;
        self.racks = next_racks;
        self.state = next_state;
        self.events.push(event.clone());
        self.private_events.push(private_event);
        Ok(event)
    }

    /// Ends the game immediately with the opposing seat as winner.
    ///
    /// # Errors
    ///
    /// Finished, wrong-seat, stale, or version failures leave state unchanged.
    pub fn resign(
        &mut self,
        player: Player,
        expected_version: u64,
    ) -> Result<GameEvent, GameError> {
        self.validate_action(player, expected_version)?;
        let updated_version = self
            .state
            .version
            .checked_add(1)
            .ok_or(GameError::VersionOverflow)?;
        let mut next_state = self.state.clone();
        next_state.version = updated_version;
        let result = self.complete_state(
            &mut next_state,
            &self.racks,
            TerminalReason::Resignation { resigned: player },
        )?;
        let event = GameEvent {
            sequence: updated_version,
            visibility: EventVisibility::Public,
            lexicon: next_state.lexicon.clone(),
            kind: GameEventKind::Resigned {
                player,
                result: result.clone(),
            },
        };
        self.state = next_state;
        self.events.push(event.clone());
        Ok(event)
    }

    /// Replays every public and private transition from a post-game seed reveal.
    ///
    /// # Errors
    ///
    /// Rejects absent/substituted packs, seed substitutions, malformed event
    /// ordering, and any public or private recomputation mismatch.
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
        if bundle.ruleset_identity != bundle.ruleset.identity() {
            return Err(GameError::InvalidTileState {
                reason: "replay ruleset identity differs from embedded ruleset".to_owned(),
            });
        }
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
        let GameEventKind::Created {
            game_id,
            ruleset,
            rng_algorithm,
            seed_commitment,
            ..
        } = &created.kind
        else {
            return Err(GameError::InvalidReplayEvent {
                sequence: created.sequence,
                reason: "first event must create the game",
            });
        };
        if created.sequence != 0
            || ruleset != &bundle.ruleset
            || created.lexicon != bundle.lexicon
            || *rng_algorithm != bundle.rng_algorithm
            || seed_commitment.algorithm != bundle.rng_algorithm
        {
            return Err(GameError::ReplayEventMismatch {
                sequence: created.sequence,
            });
        }
        let seed = GameSeed::from_bytes(bundle.seed_reveal);
        if !seed_commitment.verify(&seed) {
            return Err(GameError::SeedCommitmentMismatch);
        }
        let mut game = Self::create(game_id.clone(), bundle.ruleset.clone(), Some(lexicon), seed)?;
        if game.events[0] != *created {
            return Err(GameError::ReplayEventMismatch { sequence: 0 });
        }
        for expected in &bundle.events[1..] {
            game.replay_event(bundle, expected)?;
        }
        if game.private_events.len() != bundle.private_events.len() {
            return Err(GameError::InvalidReplayEvent {
                sequence: game.state.version,
                reason: "replay contains unmatched private transitions",
            });
        }
        Ok(game)
    }

    fn replay_event(
        &mut self,
        bundle: &ReplayBundle,
        expected: &GameEvent,
    ) -> Result<(), GameError> {
        if expected.lexicon != bundle.lexicon {
            return Err(GameError::ReplayEventMismatch {
                sequence: expected.sequence,
            });
        }
        let expects_private = match &expected.kind {
            GameEventKind::Created { .. } => {
                return Err(GameError::InvalidReplayEvent {
                    sequence: expected.sequence,
                    reason: "creation may occur only once",
                });
            }
            GameEventKind::MovePlayed {
                player, placements, ..
            } => {
                self.play_tiles(*player, self.state.version, placements.clone())?;
                Some("placement requires one private transition")
            }
            GameEventKind::Passed { player, .. } => {
                self.pass(*player, self.state.version)?;
                None
            }
            GameEventKind::Exchanged {
                player, tile_ids, ..
            } => {
                self.exchange_tiles(*player, self.state.version, tile_ids.clone())?;
                Some("exchange requires one private transition")
            }
            GameEventKind::Resigned { player, .. } => {
                self.resign(*player, self.state.version)?;
                None
            }
        };
        if let Some(reason) = expects_private {
            let expected_private = bundle
                .private_events
                .iter()
                .find(|event| event.sequence == expected.sequence)
                .ok_or(GameError::InvalidReplayEvent {
                    sequence: expected.sequence,
                    reason,
                })?;
            if self.private_events.last() != Some(expected_private) {
                return Err(GameError::ReplayEventMismatch {
                    sequence: expected.sequence,
                });
            }
        }
        if self.events.last() != Some(expected) {
            return Err(GameError::ReplayEventMismatch {
                sequence: expected.sequence,
            });
        }
        Ok(())
    }

    /// Current immutable public state.
    #[must_use]
    pub const fn public_state(&self) -> &PublicGameState {
        &self.state
    }

    /// Current private rack for one authenticated seat projection.
    #[must_use]
    pub fn rack(&self, seat: Seat) -> &Rack {
        &self.racks[seat.index()]
    }

    /// Public events emitted by this instance.
    #[must_use]
    pub fn events(&self) -> &[GameEvent] {
        &self.events
    }

    /// Seat-private transitions emitted by this instance.
    pub fn private_events(&self, seat: Seat) -> impl Iterator<Item = &PrivateGameEvent> {
        self.private_events
            .iter()
            .filter(move |event| event.seat == seat)
    }

    /// Builds the role-neutral public projection.
    #[must_use]
    pub fn public_projection(&self) -> PublicProjection {
        PublicProjection {
            schema_version: PROJECTION_SCHEMA_VERSION,
            state: self.state.clone(),
            events: self.events.clone(),
        }
    }

    /// Builds one competitive seat projection without the opponent rack.
    #[must_use]
    pub fn seat_projection(&self, seat: Seat) -> SeatProjection {
        SeatProjection {
            schema_version: PROJECTION_SCHEMA_VERSION,
            seat,
            public: self.public_projection(),
            rack: self.racks[seat.index()].clone(),
            private_events: self.private_events(seat).cloned().collect(),
        }
    }

    /// Builds the explicitly human-only full-rack spectator projection.
    #[must_use]
    pub fn human_spectator_projection(&self) -> HumanSpectatorProjection {
        HumanSpectatorProjection {
            schema_version: PROJECTION_SCHEMA_VERSION,
            public: self.public_projection(),
            racks: self.racks.clone(),
            private_events: self.private_events.clone(),
        }
    }

    /// Builds the trusted operator projection with authoritative private data.
    #[must_use]
    pub fn administrator_projection(&self) -> AdministratorProjection {
        AdministratorProjection {
            schema_version: PROJECTION_SCHEMA_VERSION,
            snapshot: self.snapshot(),
        }
    }

    /// Creates an authoritative checkpoint containing private state.
    #[must_use]
    pub fn snapshot(&self) -> GameSnapshot {
        GameSnapshot {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            ruleset: self.ruleset.identity(),
            rng_algorithm: self.state.rng_algorithm,
            state: self.state.clone(),
            bag: self.bag.clone(),
            racks: self.racks.clone(),
            seed: *self.seed.as_bytes(),
            events: self.events.clone(),
            private_events: self.private_events.clone(),
        }
    }

    /// Creates a portable replay bundle only after the game is finished.
    #[must_use]
    pub fn replay_bundle(&self) -> Option<ReplayBundle> {
        (self.state.phase == GamePhase::Finished
            && matches!(
                self.events.first().map(|event| &event.kind),
                Some(GameEventKind::Created { .. })
            ))
        .then(|| ReplayBundle {
            schema_version: REPLAY_SCHEMA_VERSION,
            ruleset_identity: self.ruleset.identity(),
            ruleset: self.ruleset.clone(),
            lexicon: self.state.lexicon.clone(),
            rng_algorithm: self.state.rng_algorithm,
            seed_reveal: *self.seed.as_bytes(),
            events: self.events.clone(),
            private_events: self.private_events.clone(),
        })
    }

    /// Returns the immutable result after completion.
    #[must_use]
    pub fn result(&self) -> Option<GameResult> {
        self.state.result.clone()
    }

    fn validate_action(&self, player: Player, expected_version: u64) -> Result<(), GameError> {
        if self.state.phase == GamePhase::Finished {
            return Err(GameError::GameFinished);
        }
        if player != self.state.current_player {
            return Err(GameError::WrongPlayer {
                expected: self.state.current_player,
                actual: player,
            });
        }
        if expected_version != self.state.version {
            return Err(GameError::StaleVersion {
                expected: self.state.version,
                actual: expected_version,
            });
        }
        Ok(())
    }

    fn complete_state(
        &self,
        state: &mut PublicGameState,
        racks: &[Rack; 2],
        reason: TerminalReason,
    ) -> Result<GameResult, GameError> {
        match reason {
            TerminalReason::Resignation { .. } => {}
            TerminalReason::ScorelessTurns => {
                for seat in Seat::ALL {
                    let deduction = self.rack_value(&racks[seat.index()])?;
                    state.scores[seat.index()] = state.scores[seat.index()]
                        .checked_add(-deduction)
                        .ok_or(GameError::ScoreOverflow)?;
                }
            }
            TerminalReason::RackEmptied { outgoing } => {
                let opponent = outgoing.opponent();
                let deduction = self.rack_value(&racks[opponent.index()])?;
                state.scores[opponent.index()] = state.scores[opponent.index()]
                    .checked_add(-deduction)
                    .ok_or(GameError::ScoreOverflow)?;
                state.scores[outgoing.index()] = state.scores[outgoing.index()]
                    .checked_add(deduction)
                    .ok_or(GameError::ScoreOverflow)?;
            }
        }
        state.phase = GamePhase::Finished;
        let winner = match reason {
            TerminalReason::Resignation { resigned } => Some(resigned.opponent()),
            TerminalReason::ScorelessTurns | TerminalReason::RackEmptied { .. } => {
                match state.scores[0].cmp(&state.scores[1]) {
                    std::cmp::Ordering::Greater => Some(Player::One),
                    std::cmp::Ordering::Less => Some(Player::Two),
                    std::cmp::Ordering::Equal => None,
                }
            }
        };
        let result = GameResult {
            game_id: self.state.game_id.clone(),
            ruleset_id: state.ruleset_id,
            lexicon: state.lexicon.clone(),
            scores: state.scores,
            winner,
            final_version: state.version,
            reason,
        };
        state.result = Some(result.clone());
        Ok(result)
    }

    fn rack_value(&self, rack: &Rack) -> Result<i32, GameError> {
        rack.tiles().iter().try_fold(0_i32, |total, tile| {
            let value = match &tile.face {
                TileFace::Letter(token) => {
                    i32::from(self.ruleset.letter_value(token.as_str()).unwrap_or(0))
                }
                TileFace::Blank => 0,
            };
            total.checked_add(value).ok_or(GameError::ScoreOverflow)
        })
    }

    fn prepare_placement(
        &self,
        player: Player,
        mut placements: Vec<Placement>,
    ) -> Result<PreparedMove, GameError> {
        if placements.is_empty() {
            return Err(GameError::EmptyPlacement);
        }
        let played_by_id = self.validate_owned_placements(player, &mut placements)?;
        Self::validate_unique_coordinates(&placements)?;
        let orientation = orientation(&placements)?;
        placements.sort_unstable_by_key(|placement| match orientation {
            Orientation::Horizontal => (placement.coordinate.column, placement.coordinate.row),
            Orientation::Vertical | Orientation::Single => {
                (placement.coordinate.row, placement.coordinate.column)
            }
        });
        let proposed = placements
            .iter()
            .map(|placement| (placement.coordinate, placement))
            .collect::<BTreeMap<_, _>>();
        self.validate_contiguous(&placements, orientation, &proposed)?;
        self.validate_connection(&proposed)?;
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
        let bingo_bonus = if placements.len() == usize::from(self.ruleset.game.rack_capacity) {
            u32::from(self.ruleset.game.bingo_bonus)
        } else {
            0
        };
        score = score
            .checked_add(bingo_bonus)
            .ok_or(GameError::ScoreOverflow)?;
        let owned_tiles = placements
            .iter()
            .map(|placement| {
                played_by_id
                    .get(&placement.tile_id)
                    .expect("validated owned tile")
                    .clone()
            })
            .collect();
        Ok(PreparedMove {
            placements,
            played: owned_tiles,
            words,
            bingo_bonus,
            score,
        })
    }

    fn validate_owned_placements(
        &self,
        player: Player,
        placements: &mut [Placement],
    ) -> Result<BTreeMap<TileId, PhysicalTile>, GameError> {
        let rack = &self.racks[player.index()];
        let mut tile_ids = BTreeSet::new();
        let mut played_by_id = BTreeMap::new();
        for placement in placements {
            if !tile_ids.insert(placement.tile_id) {
                return Err(GameError::DuplicatePlacementTile {
                    tile_id: placement.tile_id,
                });
            }
            let physical = rack
                .tiles()
                .iter()
                .find(|tile| tile.id == placement.tile_id)
                .ok_or(GameError::TileNotOwned {
                    tile_id: placement.tile_id,
                })?;
            if !placement.coordinate.in_bounds(BOARD_SIZE, BOARD_SIZE) {
                return Err(GameError::CoordinateOutOfBounds {
                    coordinate: placement.coordinate,
                });
            }
            placement.tile.letter = canonical_tile_token(
                &self.ruleset.lexicon.normalization.profile,
                &placement.tile.letter,
            )?;
            validate_assignment(physical, &placement.tile)?;
            if self.state.tile_at(placement.coordinate).is_some() {
                return Err(GameError::OccupiedSquare {
                    coordinate: placement.coordinate,
                });
            }
            played_by_id.insert(physical.id, physical.clone());
        }
        Ok(played_by_id)
    }

    fn validate_unique_coordinates(placements: &[Placement]) -> Result<(), GameError> {
        let unique_coordinates = placements
            .iter()
            .map(|placement| placement.coordinate)
            .collect::<BTreeSet<_>>();
        if unique_coordinates.len() != placements.len() {
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
        Ok(())
    }

    fn validate_contiguous(
        &self,
        placements: &[Placement],
        orientation: Orientation,
        proposed: &BTreeMap<Coordinate, &Placement>,
    ) -> Result<(), GameError> {
        if orientation == Orientation::Single {
            return Ok(());
        }
        let first = placements.first().expect("nonempty placement").coordinate;
        let last = placements.last().expect("nonempty placement").coordinate;
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
        proposed: &BTreeMap<Coordinate, &Placement>,
    ) -> Result<(), GameError> {
        let board_is_empty = self.state.board.iter().all(Option::is_none);
        if board_is_empty {
            return proposed
                .contains_key(&CENTER)
                .then_some(())
                .ok_or(GameError::OpeningMoveMissesCenter);
        }
        if proposed.keys().any(|coordinate| {
            neighbors(*coordinate)
                .into_iter()
                .flatten()
                .any(|neighbor| self.state.tile_at(neighbor).is_some())
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
        proposed: &BTreeMap<Coordinate, &Placement>,
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
        proposed: &BTreeMap<Coordinate, &Placement>,
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
        proposed: &BTreeMap<Coordinate, &Placement>,
    ) -> Result<FormedWord, GameError> {
        let mut text = String::new();
        let mut letter_score = 0_u32;
        let mut word_multiplier = 1_u32;
        for coordinate in &coordinates {
            let tile = self
                .tile_with_proposed(*coordinate, proposed)
                .expect("formed word coordinates are occupied");
            text.push_str(tile.letter());
            let mut value = if tile.is_blank() {
                0
            } else {
                u32::from(self.ruleset.letter_value(tile.letter()).unwrap_or(0))
            };
            if proposed.contains_key(coordinate) {
                let premium = self
                    .ruleset
                    .game
                    .board
                    .square(*coordinate)
                    .expect("ruleset board is complete")
                    .premium;
                match premium {
                    Premium::Normal => {}
                    Premium::DoubleLetter => {
                        value = value.checked_mul(2).ok_or(GameError::ScoreOverflow)?;
                    }
                    Premium::TripleLetter => {
                        value = value.checked_mul(3).ok_or(GameError::ScoreOverflow)?;
                    }
                    Premium::DoubleWord => {
                        word_multiplier = word_multiplier
                            .checked_mul(2)
                            .ok_or(GameError::ScoreOverflow)?;
                    }
                    Premium::TripleWord => {
                        word_multiplier = word_multiplier
                            .checked_mul(3)
                            .ok_or(GameError::ScoreOverflow)?;
                    }
                }
            }
            letter_score = letter_score
                .checked_add(value)
                .ok_or(GameError::ScoreOverflow)?;
        }
        let normalized = normalize_key(&self.ruleset.lexicon.normalization.profile, &text)?;
        if !self.lexicon.contains(&normalized) {
            return Err(GameError::InvalidWord {
                word: text,
                normalized: normalized.into_string(),
            });
        }
        let score = letter_score
            .checked_mul(word_multiplier)
            .ok_or(GameError::ScoreOverflow)?;
        Ok(FormedWord {
            text,
            normalized: normalized.into_string(),
            coordinates,
            letter_score,
            word_multiplier,
            score,
        })
    }

    fn tile_with_proposed<'a>(
        &'a self,
        coordinate: Coordinate,
        proposed: &'a BTreeMap<Coordinate, &Placement>,
    ) -> Option<TileRef<'a>> {
        proposed
            .get(&coordinate)
            .map(|placement| TileRef::Proposed(placement))
            .or_else(|| self.state.tile_at(coordinate).map(TileRef::Board))
    }
}

struct PreparedMove {
    placements: Vec<Placement>,
    played: Vec<PhysicalTile>,
    words: Vec<FormedWord>,
    bingo_bonus: u32,
    score: u32,
}

enum TileRef<'a> {
    Proposed(&'a Placement),
    Board(&'a BoardTile),
}

impl TileRef<'_> {
    fn letter(&self) -> &str {
        match self {
            Self::Proposed(placement) => &placement.tile.letter,
            Self::Board(tile) => &tile.letter,
        }
    }

    const fn is_blank(&self) -> bool {
        match self {
            Self::Proposed(placement) => placement.tile.is_blank,
            Self::Board(tile) => tile.is_blank,
        }
    }
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

fn validate_assignment(physical: &PhysicalTile, assigned: &Tile) -> Result<(), GameError> {
    let matches = match &physical.face {
        TileFace::Letter(token) => !assigned.is_blank && assigned.letter == token.as_str(),
        TileFace::Blank => assigned.is_blank,
    };
    if matches {
        Ok(())
    } else {
        Err(GameError::TileFaceMismatch {
            tile_id: physical.id,
        })
    }
}

fn physical_board(
    state: &PublicGameState,
    ruleset: &Ruleset,
) -> Result<Vec<PhysicalTile>, GameError> {
    state
        .board
        .iter()
        .flatten()
        .map(|tile| {
            let canonical =
                canonical_tile_token(&ruleset.lexicon.normalization.profile, &tile.letter)?;
            if canonical != tile.letter {
                return Err(GameError::NonCanonicalBoardTile {
                    token: tile.letter.clone(),
                    canonical,
                });
            }
            let face = if tile.is_blank {
                TileFace::Blank
            } else {
                TileFace::Letter(TileToken::new(tile.letter.clone()).map_err(|_| {
                    GameError::InvalidTileState {
                        reason: format!(
                            "board tile {:?} has an invalid physical token",
                            tile.tile_id
                        ),
                    }
                })?)
            };
            Ok(PhysicalTile {
                id: tile.tile_id,
                face,
            })
        })
        .collect()
}

fn rack_counts(racks: &[Rack; 2]) -> Result<[u8; 2], GameError> {
    Ok([count_u8(racks[0].len())?, count_u8(racks[1].len())?])
}

fn count_u8(count: usize) -> Result<u8, GameError> {
    u8::try_from(count).map_err(|_| GameError::InvalidTileState {
        reason: format!("tile count {count} exceeds u8"),
    })
}

fn count_u16(count: usize) -> Result<u16, GameError> {
    u16::try_from(count).map_err(|_| GameError::InvalidTileState {
        reason: format!("tile count {count} exceeds u16"),
    })
}

fn tile_state_error(error: impl std::fmt::Display) -> GameError {
    GameError::InvalidTileState {
        reason: error.to_string(),
    }
}

fn next_scoreless(current: u8, score: u32) -> Result<u8, GameError> {
    if score == 0 {
        current
            .checked_add(1)
            .ok_or(GameError::ScorelessTurnOverflow)
    } else {
        Ok(0)
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
