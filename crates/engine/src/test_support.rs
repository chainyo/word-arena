//! Opt-in deterministic match tooling used by tests and engine benchmarks.
//!
//! The `test-support` feature is intentionally disabled by default. Application
//! transports must drive [`crate::Game`] directly and must not publish these
//! baseline move-generation helpers as competitive agent tools.

use std::{collections::BTreeMap, sync::Arc};

use sha2::{Digest, Sha256};
use thiserror::Error;
use word_arena_lexicon::normalize_key;

use crate::{
    Coordinate, Game, GameError, GamePhase, GameResult, GameSeed, GameSnapshot,
    HumanSpectatorProjection, Move, PhysicalTile, Placement, Player, PublicProjection,
    ReplayBundle, Ruleset, Seat, SeatProjection, Tile, TileFace, WordValidator,
};

const BOARD_SIZE: u8 = 15;
const CENTER: u8 = 7;

/// One legal action and the score it would add immediately.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MoveChoice {
    /// Action accepted by the authoritative engine at the current version.
    pub action: Move,
    /// Placement score, or zero for pass and exchange.
    pub immediate_score: u32,
}

/// Deterministic move generator fed by a small candidate-word catalog.
///
/// Candidate enumeration lives only in this feature-gated verification module.
/// The authoritative engine still validates every returned action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MoveGenerator {
    words: Vec<String>,
    rack_probes: bool,
}

impl MoveGenerator {
    /// Builds a stable deduplicated catalog using the ruleset's board-key
    /// normalization. Words that cannot fit on the V1 board are ignored.
    ///
    /// # Errors
    ///
    /// Returns the engine normalization error for an incompatible candidate.
    pub fn new(
        ruleset: &Ruleset,
        words: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Self, GameError> {
        let mut words = words
            .into_iter()
            .map(|word| {
                normalize_key(&ruleset.lexicon.normalization.profile, word.as_ref())
                    .map(word_arena_lexicon::NormalizedKey::into_string)
                    .map_err(GameError::from)
            })
            .collect::<Result<Vec<_>, _>>()?;
        words.retain(|word| (2..=usize::from(BOARD_SIZE)).contains(&word.len()));
        words.sort();
        words.dedup();
        Ok(Self {
            words,
            rack_probes: false,
        })
    }

    /// Enables rack-derived probes for broad tests with an accepting fixture
    /// validator. Real pack scenarios should use candidate words instead.
    #[must_use]
    pub const fn with_rack_probes(mut self) -> Self {
        self.rack_probes = true;
        self
    }

    /// Enumerates legal placements, one-tile exchanges, and pass in a stable
    /// order. Each placement is previewed by the authoritative validator and
    /// scorer without mutating the game.
    #[must_use]
    pub fn legal_choices(&self, game: &Game, player: Player) -> Vec<MoveChoice> {
        if game.public_state().phase == GamePhase::Finished
            || game.public_state().current_player != player
        {
            return Vec::new();
        }

        let mut choices = BTreeMap::new();
        for word in &self.words {
            Self::add_word_candidates(game, player, word, &mut choices);
        }
        if self.rack_probes {
            Self::add_rack_probes(game, player, &mut choices);
        }
        if game.public_state().bag_count >= game.ruleset().game.exchange_minimum {
            for tile in game.rack(player).tiles() {
                insert_choice(
                    &mut choices,
                    MoveChoice {
                        action: Move::Exchange {
                            tile_ids: vec![tile.id],
                        },
                        immediate_score: 0,
                    },
                );
            }
        }
        insert_choice(
            &mut choices,
            MoveChoice {
                action: Move::Pass,
                immediate_score: 0,
            },
        );
        choices.into_values().collect()
    }

    fn add_word_candidates(
        game: &Game,
        player: Player,
        word: &str,
        choices: &mut BTreeMap<ActionKey, MoveChoice>,
    ) {
        let length = u8::try_from(word.len()).expect("catalog words fit the V1 board");
        for orientation in [Orientation::Horizontal, Orientation::Vertical] {
            let (row_count, column_count) = match orientation {
                Orientation::Horizontal => (BOARD_SIZE, BOARD_SIZE - length + 1),
                Orientation::Vertical => (BOARD_SIZE - length + 1, BOARD_SIZE),
            };
            for row in 0..row_count {
                for column in 0..column_count {
                    let start = Coordinate::new(row, column);
                    for placements in assignments_for_word(game, player, word, start, orientation) {
                        add_if_legal(game, player, placements, choices);
                    }
                }
            }
        }
    }

    fn add_rack_probes(game: &Game, player: Player, choices: &mut BTreeMap<ActionKey, MoveChoice>) {
        if game.public_state().board.iter().all(Option::is_none) {
            let rack = game.rack(player).tiles();
            for (left_index, left) in rack.iter().enumerate() {
                for (right_index, right) in rack.iter().enumerate() {
                    if left_index == right_index {
                        continue;
                    }
                    for left_tile in assignments(left) {
                        for right_tile in assignments(right) {
                            add_if_legal(
                                game,
                                player,
                                vec![
                                    Placement::new(
                                        left.id,
                                        Coordinate::new(CENTER, CENTER - 1),
                                        left_tile.clone(),
                                    ),
                                    Placement::new(
                                        right.id,
                                        Coordinate::new(CENTER, CENTER),
                                        right_tile,
                                    ),
                                ],
                                choices,
                            );
                        }
                    }
                }
            }
            return;
        }

        for row in 0..BOARD_SIZE {
            for column in 0..BOARD_SIZE {
                let coordinate = Coordinate::new(row, column);
                if game.public_state().tile_at(coordinate).is_none()
                    || !has_empty_neighbor(game, coordinate)
                {
                    continue;
                }
                for target in neighbors(coordinate)
                    .into_iter()
                    .flatten()
                    .filter(|target| game.public_state().tile_at(*target).is_none())
                {
                    for physical in game.rack(player).tiles() {
                        for tile in assignments(physical) {
                            add_if_legal(
                                game,
                                player,
                                vec![Placement::new(physical.id, target, tile)],
                                choices,
                            );
                        }
                    }
                }
            }
        }
    }
}

/// Baseline deterministic policy used only by the in-memory verification
/// runner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BotStrategy {
    /// Select any generated legal action through a stable SHA-256 choice.
    RandomLegal {
        /// Bot-specific entropy; equal inputs always select the same action.
        seed: [u8; 32],
    },
    /// Select the highest immediate placement score, then the first stable key.
    Greedy,
}

impl BotStrategy {
    /// Chooses one action from a generator's stable legal action list.
    #[must_use]
    pub fn choose(
        self,
        game: &Game,
        player: Player,
        generator: &MoveGenerator,
    ) -> Option<MoveChoice> {
        let choices = generator.legal_choices(game, player);
        match self {
            Self::Greedy => {
                let maximum = choices.iter().map(|choice| choice.immediate_score).max()?;
                choices
                    .into_iter()
                    .find(|choice| choice.immediate_score == maximum)
            }
            Self::RandomLegal { seed } => {
                if choices.is_empty() {
                    return None;
                }
                let mut hash = Sha256::new();
                hash.update(b"word-arena-random-legal-bot-v1\0");
                hash.update(seed);
                hash.update(game.public_state().game_id.as_bytes());
                hash.update(game.public_state().version.to_be_bytes());
                hash.update([player.number()]);
                let digest = hash.finalize();
                let index = u64::from_be_bytes([
                    digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6],
                    digest[7],
                ]) % u64::try_from(choices.len()).ok()?;
                choices.into_iter().nth(usize::try_from(index).ok()?)
            }
        }
    }
}

/// Complete inputs for one deterministic in-memory match.
#[derive(Clone, Debug)]
pub struct MatchSpec {
    /// Stable game identifier included in events and bot choice hashing.
    pub game_id: String,
    /// Exact physical rules and lexicon identity.
    pub ruleset: Ruleset,
    /// Immutable exact-membership validator.
    pub lexicon: Arc<dyn WordValidator>,
    /// Private deterministic game seed.
    pub seed: GameSeed,
    /// Feature-gated candidate move generator.
    pub generator: MoveGenerator,
    /// Strategies for seats one and two.
    pub bots: [BotStrategy; 2],
    /// Hard bound protecting tests from nontermination.
    pub max_turns: u64,
}

/// Byte-serializable artifacts returned after a fully verified match.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MatchOutcome {
    /// Number of accepted actions, excluding game creation.
    pub turns: u64,
    /// Immutable terminal result.
    pub result: GameResult,
    /// Final authoritative checkpoint.
    pub snapshot: GameSnapshot,
    /// Portable post-game deterministic replay.
    pub replay: ReplayBundle,
    /// Final public role projection.
    pub public: PublicProjection,
    /// Final private seat projections in stable seat order.
    pub seats: [SeatProjection; 2],
    /// Final trusted-human spectator projection.
    pub spectator: HumanSpectatorProjection,
}

/// Bounded in-memory runner failure.
#[derive(Debug, Error)]
pub enum MatchError {
    /// Authoritative engine validation failed.
    #[error(transparent)]
    Game(#[from] GameError),
    /// A bot or generator produced no action for an active game.
    #[error("no generated action for active seat {seat:?} at version {version}")]
    NoAction {
        /// Seat that could not act.
        seat: Seat,
        /// Active authoritative version.
        version: u64,
    },
    /// The deterministic match exceeded its configured action bound.
    #[error("match did not finish within {limit} accepted actions")]
    TurnLimit {
        /// Configured maximum accepted actions.
        limit: u64,
    },
    /// A finished game did not produce a replay artifact.
    #[error("finished match did not produce a replay bundle")]
    MissingReplay,
    /// Snapshot resume or replay did not reproduce the final checkpoint.
    #[error("final resume or replay differs from the authoritative match")]
    ReproductionMismatch,
}

/// Runs bots until a terminal state, then proves final snapshot resume and
/// seed-reveal replay equivalence through public engine APIs.
///
/// # Errors
///
/// Returns an engine error, an absent action, a turn-limit failure, or a final
/// deterministic reproduction mismatch.
pub fn run_match(spec: MatchSpec) -> Result<MatchOutcome, MatchError> {
    let MatchSpec {
        game_id,
        ruleset,
        lexicon,
        seed,
        generator,
        bots,
        max_turns,
    } = spec;
    let mut game = Game::create(game_id, ruleset.clone(), Some(Arc::clone(&lexicon)), seed)?;
    while game.public_state().phase == GamePhase::Active {
        if game.public_state().version >= max_turns {
            return Err(MatchError::TurnLimit { limit: max_turns });
        }
        let player = game.public_state().current_player;
        let strategy = bots[player.index()];
        let choice = strategy
            .choose(&game, player, &generator)
            .ok_or(MatchError::NoAction {
                seat: player,
                version: game.public_state().version,
            })?;
        game.apply_move(player, game.public_state().version, choice.action)?;
    }

    let result = game.result().ok_or(MatchError::MissingReplay)?;
    let snapshot = game.snapshot();
    let replay = game.replay_bundle().ok_or(MatchError::MissingReplay)?;
    let resumed = Game::resume(
        snapshot.clone(),
        ruleset.clone(),
        Some(Arc::clone(&lexicon)),
    )?;
    let replayed = Game::replay(&replay, Some(lexicon))?;
    if resumed.snapshot() != snapshot || replayed.snapshot() != snapshot {
        return Err(MatchError::ReproductionMismatch);
    }

    Ok(MatchOutcome {
        turns: snapshot.state.version,
        result,
        public: game.public_projection(),
        seats: [
            game.seat_projection(Seat::One),
            game.seat_projection(Seat::Two),
        ],
        spectator: game.human_spectator_projection(),
        snapshot,
        replay,
    })
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ActionKey {
    Place(Vec<(u8, u8, u16, String, bool)>),
    Exchange(Vec<u16>),
    Pass,
    Resign,
}

#[derive(Clone, Copy)]
enum Orientation {
    Horizontal,
    Vertical,
}

fn insert_choice(choices: &mut BTreeMap<ActionKey, MoveChoice>, choice: MoveChoice) {
    choices.entry(action_key(&choice.action)).or_insert(choice);
}

fn action_key(action: &Move) -> ActionKey {
    match action {
        Move::Place { placements } => ActionKey::Place(
            placements
                .iter()
                .map(|placement| {
                    (
                        placement.coordinate.row,
                        placement.coordinate.column,
                        placement.tile_id.0,
                        placement.tile.letter.clone(),
                        placement.tile.is_blank,
                    )
                })
                .collect(),
        ),
        Move::Exchange { tile_ids } => {
            ActionKey::Exchange(tile_ids.iter().map(|tile_id| tile_id.0).collect())
        }
        Move::Pass => ActionKey::Pass,
        Move::Resign => ActionKey::Resign,
    }
}

fn add_if_legal(
    game: &Game,
    player: Player,
    placements: Vec<Placement>,
    choices: &mut BTreeMap<ActionKey, MoveChoice>,
) {
    if let Ok(score) = game.preview_placement(player, placements.clone()) {
        insert_choice(
            choices,
            MoveChoice {
                action: Move::Place { placements },
                immediate_score: score,
            },
        );
    }
}

fn assignments_for_word(
    game: &Game,
    player: Player,
    word: &str,
    start: Coordinate,
    orientation: Orientation,
) -> Vec<Vec<Placement>> {
    let mut needs = Vec::new();
    for (offset, letter) in word.bytes().enumerate() {
        let offset = u8::try_from(offset).expect("catalog word fits V1 board");
        let coordinate = match orientation {
            Orientation::Horizontal => Coordinate::new(start.row, start.column + offset),
            Orientation::Vertical => Coordinate::new(start.row + offset, start.column),
        };
        let letter = char::from(letter).to_string();
        if let Some(existing) = game.public_state().tile_at(coordinate) {
            if existing.letter != letter {
                return Vec::new();
            }
        } else {
            needs.push((coordinate, letter));
        }
    }
    if needs.is_empty() || needs.len() > game.rack(player).len() {
        return Vec::new();
    }

    let mut results = Vec::new();
    assign_needs(
        game.rack(player).tiles(),
        &needs,
        0,
        &mut vec![false; game.rack(player).len()],
        &mut Vec::new(),
        &mut results,
    );
    results
}

fn assign_needs(
    rack: &[PhysicalTile],
    needs: &[(Coordinate, String)],
    need_index: usize,
    used: &mut [bool],
    placements: &mut Vec<Placement>,
    results: &mut Vec<Vec<Placement>>,
) {
    if need_index == needs.len() {
        results.push(placements.clone());
        return;
    }
    let (coordinate, letter) = &needs[need_index];
    let regular = rack.iter().enumerate().find(|(index, tile)| {
        !used[*index] && matches!(&tile.face, TileFace::Letter(token) if token.as_str() == letter)
    });
    let blank = rack
        .iter()
        .enumerate()
        .find(|(index, tile)| !used[*index] && matches!(tile.face, TileFace::Blank));
    for (index, physical) in regular.into_iter().chain(blank) {
        used[index] = true;
        let tile = match physical.face {
            TileFace::Letter(_) => Tile::letter(letter.clone()),
            TileFace::Blank => Tile::blank(letter.clone()),
        };
        placements.push(Placement::new(physical.id, *coordinate, tile));
        assign_needs(rack, needs, need_index + 1, used, placements, results);
        placements.pop();
        used[index] = false;
    }
}

fn assignments(tile: &PhysicalTile) -> Vec<Tile> {
    match &tile.face {
        TileFace::Letter(token) => vec![Tile::letter(token.as_str())],
        TileFace::Blank => ('A'..='Z')
            .map(|letter| Tile::blank(letter.to_string()))
            .collect(),
    }
}

fn has_empty_neighbor(game: &Game, coordinate: Coordinate) -> bool {
    neighbors(coordinate)
        .into_iter()
        .flatten()
        .any(|target| game.public_state().tile_at(target).is_none())
}

fn neighbors(coordinate: Coordinate) -> [Option<Coordinate>; 4] {
    [
        coordinate
            .row
            .checked_sub(1)
            .map(|row| Coordinate::new(row, coordinate.column)),
        (coordinate.row + 1 < BOARD_SIZE)
            .then(|| Coordinate::new(coordinate.row + 1, coordinate.column)),
        coordinate
            .column
            .checked_sub(1)
            .map(|column| Coordinate::new(coordinate.row, column)),
        (coordinate.column + 1 < BOARD_SIZE)
            .then(|| Coordinate::new(coordinate.row, coordinate.column + 1)),
    ]
}
