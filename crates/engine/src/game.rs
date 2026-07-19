use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use word_arena_lexicon::{CompatibilityContext, PackIdentity, normalize_key};

use crate::{
    Bag, Coordinate, GameError, GameSeed, PhysicalTile, Player, Premium, Rack, RngAlgorithm,
    Ruleset, RulesetId, Seat, SeedCommitment, TileFace, TileId, TileToken, WordValidator,
    prepare_initial_deal, verify_tile_conservation,
};

/// Width and height of the V1 board.
pub const BOARD_SIZE: u8 = 15;
const BOARD_SQUARES: usize = BOARD_SIZE as usize * BOARD_SIZE as usize;
const CENTER: Coordinate = Coordinate { row: 7, column: 7 };
const SNAPSHOT_SCHEMA_VERSION: u32 = 2;
const REPLAY_SCHEMA_VERSION: u32 = 2;

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
    /// Exact pack selected during creation.
    pub lexicon: PackIdentity,
    /// Public seed commitment; the seed remains private during play.
    pub seed_commitment: SeedCommitment,
    /// Row-major 15x15 public board.
    pub board: Vec<Option<BoardTile>>,
    /// Scores for seats one and two.
    pub scores: [u32; 2],
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
    /// Complete public state.
    pub state: PublicGameState,
    /// Exact private bag order.
    pub bag: Bag,
    /// Exact private racks in seat order.
    pub racks: [Rack; 2],
    /// Private seed retained for resume and post-game replay reveal.
    pub seed: [u8; 32],
}

impl std::fmt::Debug for GameSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GameSnapshot")
            .field("schema_version", &self.schema_version)
            .field("state", &self.state)
            .field("bag", &"[REDACTED]")
            .field("racks", &"[REDACTED]")
            .field("seed", &"[REDACTED]")
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
    pub scores: [u32; 2],
    /// Winning seat, or `None` for a tie.
    pub winner: Option<Player>,
    /// Final state version.
    pub final_version: u64,
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
        scores_after: [u32; 2],
        /// Scoreless counter after commit.
        scoreless_turns_after: u8,
        /// Next active seat.
        next_player: Player,
    },
    /// Explicit immutable completion.
    Finished {
        /// Final result, including the exact pack identity.
        result: GameResult,
    },
}

/// Ordered immutable public game event.
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

/// Seat-private tile transition paired with one public move event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateGameEvent {
    /// Matching public event sequence.
    pub sequence: u64,
    /// Only this seat may receive the projection during live play.
    pub seat: Seat,
    /// Exact owned physical tiles removed by the move.
    pub played: Vec<PhysicalTile>,
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
    /// Full versioned ruleset.
    pub ruleset: Ruleset,
    /// Exact pack required to replay.
    pub lexicon: PackIdentity,
    /// Post-game seed reveal.
    pub seed_reveal: [u8; 32],
    /// Complete ordered public event stream.
    pub events: Vec<GameEvent>,
    /// Complete ordered seat-private placement transitions.
    pub private_events: Vec<PrivateGameEvent>,
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
            lexicon: identity.clone(),
            seed_commitment: commitment.clone(),
            board: vec![None; BOARD_SQUARES],
            scores: [0, 0],
            current_player: Player::One,
            version: 0,
            scoreless_turns: 0,
            rack_counts,
            bag_count,
            phase: GamePhase::Active,
        };
        let event = GameEvent {
            sequence: 0,
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
        let seed = GameSeed::from_bytes(snapshot.seed);
        if !snapshot.state.seed_commitment.verify(&seed) {
            return Err(GameError::SeedCommitmentMismatch);
        }
        let board = physical_board(&snapshot.state, &ruleset)?;
        verify_tile_conservation(&ruleset, &snapshot.bag, &snapshot.racks, &board)
            .map_err(tile_state_error)?;
        if snapshot
            .racks
            .iter()
            .any(|rack| rack.len() > usize::from(ruleset.game.rack_capacity))
        {
            return Err(GameError::InvalidTileState {
                reason: "persisted rack exceeds configured capacity".to_owned(),
            });
        }
        if snapshot.state.rack_counts != rack_counts(&snapshot.racks)?
            || snapshot.state.bag_count != count_u16(snapshot.bag.len())?
        {
            return Err(GameError::InvalidTileState {
                reason: "public ownership counts differ from authoritative locations".to_owned(),
            });
        }
        Ok(Self {
            ruleset,
            lexicon,
            seed,
            bag: snapshot.bag,
            racks: snapshot.racks,
            state: snapshot.state,
            events: Vec::new(),
            private_events: Vec::new(),
        })
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
        let prepared = self.prepare_placement(player, placements)?;
        let updated_score = self.state.scores[player.index()]
            .checked_add(prepared.score)
            .ok_or(GameError::ScoreOverflow)?;
        let updated_version = self
            .state
            .version
            .checked_add(1)
            .ok_or(GameError::VersionOverflow)?;
        let scoreless_turns = if prepared.score == 0 {
            self.state
                .scoreless_turns
                .checked_add(1)
                .ok_or(GameError::ScorelessTurnOverflow)?
        } else {
            0
        };

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

        let event = GameEvent {
            sequence: updated_version,
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
            },
        };
        let private_event = PrivateGameEvent {
            sequence: updated_version,
            seat: player,
            played: prepared.played,
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
            sequence: updated_version,
            lexicon: self.state.lexicon.clone(),
            kind: GameEventKind::Finished {
                result: result.clone(),
            },
        });
        Ok(result)
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
            seed_commitment,
            ..
        } = &created.kind
        else {
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
        let seed = GameSeed::from_bytes(bundle.seed_reveal);
        if !seed_commitment.verify(&seed) {
            return Err(GameError::SeedCommitmentMismatch);
        }
        let mut game = Self::create(game_id.clone(), bundle.ruleset.clone(), Some(lexicon), seed)?;
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
                    game.play_tiles(*player, game.state.version, placements.clone())?;
                    let expected_private = bundle
                        .private_events
                        .iter()
                        .find(|event| event.sequence == expected.sequence)
                        .ok_or(GameError::InvalidReplayEvent {
                            sequence: expected.sequence,
                            reason: "placement requires one private transition",
                        })?;
                    if game.private_events.last() != Some(expected_private) {
                        return Err(GameError::ReplayEventMismatch {
                            sequence: expected.sequence,
                        });
                    }
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
        if game.private_events.len() != bundle.private_events.len() {
            return Err(GameError::InvalidReplayEvent {
                sequence: game.state.version,
                reason: "replay contains unmatched private transitions",
            });
        }
        Ok(game)
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

    /// Creates an authoritative checkpoint containing private state.
    #[must_use]
    pub fn snapshot(&self) -> GameSnapshot {
        GameSnapshot {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            state: self.state.clone(),
            bag: self.bag.clone(),
            racks: self.racks.clone(),
            seed: *self.seed.as_bytes(),
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
            ruleset: self.ruleset.clone(),
            lexicon: self.state.lexicon.clone(),
            seed_reveal: *self.seed.as_bytes(),
            events: self.events.clone(),
            private_events: self.private_events.clone(),
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
