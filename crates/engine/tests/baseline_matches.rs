#![cfg(feature = "test-support")]

use std::{collections::BTreeSet, sync::Arc};

use proptest::prelude::*;
use serde_json::Value;
use word_arena_engine::{
    EventVisibility, Game, GameError, GameEventKind, GamePhase, GameSeed, Language, Move, Ruleset,
    Score, Seat, TerminalReason, WordValidator,
    test_support::{BotStrategy, MatchError, MatchOutcome, MatchSpec, MoveGenerator, run_match},
};
use word_arena_lexicon::{NormalizedKey, PackIdentity, normalize_key};

const ENGLISH_WORDS: &[&str] = &[
    "AN", "AS", "AT", "ATE", "BE", "BY", "DO", "EAT", "GO", "HE", "IN", "IS", "IT", "ME", "MY",
    "NO", "ON", "OR", "TEA", "TO", "UP", "US", "WE",
];
const FRENCH_WORDS: &[&str] = &[
    "DE", "DES", "DU", "EN", "ET", "ETE", "IL", "JE", "LA", "LE", "LES", "NON", "ON", "OU", "OUI",
    "TU", "UN", "UNE",
];

#[derive(Debug)]
struct FixtureLexicon {
    identity: PackIdentity,
    words: BTreeSet<String>,
    accept_all: bool,
}

impl WordValidator for FixtureLexicon {
    fn identity(&self) -> &PackIdentity {
        &self.identity
    }

    fn contains(&self, key: &NormalizedKey) -> bool {
        self.accept_all || self.words.contains(key.as_ref())
    }
}

#[test]
fn english_and_french_golden_matches_are_byte_deterministic() {
    let scenarios = [
        (
            Ruleset::english_v1(),
            ENGLISH_WORDS,
            [
                BotStrategy::RandomLegal { seed: [11; 32] },
                BotStrategy::RandomLegal { seed: [29; 32] },
            ],
            41_u64,
            (9_u64, [Score::new(-16), Score::new(-18)]),
        ),
        (
            Ruleset::french_v1(),
            FRENCH_WORDS,
            [
                BotStrategy::Greedy,
                BotStrategy::RandomLegal { seed: [37; 32] },
            ],
            73_u64,
            (17_u64, [Score::new(14), Score::new(-12)]),
        ),
    ];

    for (ruleset, words, bots, seed, expected) in scenarios {
        let first = run_fixture_match(&ruleset, words, false, bots, seed, 128).unwrap();
        let second = run_fixture_match(&ruleset, words, false, bots, seed, 128).unwrap();
        assert_eq!(first.turns, expected.0);
        assert_eq!(first.result.scores.as_slice(), expected.1.as_slice());
        assert_eq!(first.result.winner, Some(Seat::One));
        assert_eq!(first.result.reason, TerminalReason::ScorelessTurns);
        assert_eq!(outcome_bytes(&first), outcome_bytes(&second));
        assert_terminal_invariants(&first, usize::try_from(ruleset.game.total_tiles()).unwrap());
    }
}

#[test]
fn both_baseline_matchups_finish_in_both_languages() {
    for (ruleset, words) in [
        (Ruleset::english_v1(), ENGLISH_WORDS),
        (Ruleset::french_v1(), FRENCH_WORDS),
    ] {
        for bots in [
            [
                BotStrategy::RandomLegal { seed: [3; 32] },
                BotStrategy::RandomLegal { seed: [5; 32] },
            ],
            [
                BotStrategy::Greedy,
                BotStrategy::RandomLegal { seed: [7; 32] },
            ],
        ] {
            let outcome = run_fixture_match(&ruleset, words, false, bots, 101, 128).unwrap();
            assert_terminal_invariants(
                &outcome,
                usize::try_from(ruleset.game.total_tiles()).unwrap(),
            );
        }
    }
}

#[test]
fn role_privacy_event_sequence_and_pack_boundary_hold_for_complete_matches() {
    for (ruleset, words) in [
        (Ruleset::english_v1(), ENGLISH_WORDS),
        (Ruleset::french_v1(), FRENCH_WORDS),
    ] {
        let outcome = run_fixture_match(
            &ruleset,
            words,
            false,
            [BotStrategy::Greedy, BotStrategy::Greedy],
            127,
            128,
        )
        .unwrap();
        let public = serde_json::to_value(&outcome.public).unwrap();
        assert!(!contains_key(&public, "seed"));
        assert!(!contains_key(&public, "bag"));
        assert!(!contains_key(&public, "rack"));
        assert!(!contains_key(&public, "private_events"));
        assert!(
            outcome
                .public
                .events
                .iter()
                .all(|event| event.visibility == EventVisibility::Public)
        );
        assert_eq!(outcome.seats[0].rack, outcome.snapshot.racks[0]);
        assert_eq!(outcome.seats[1].rack, outcome.snapshot.racks[1]);
        assert!(
            outcome.seats[0]
                .private_events
                .iter()
                .all(|event| event.visibility == EventVisibility::SeatPrivate(Seat::One))
        );
        assert!(
            outcome.seats[1]
                .private_events
                .iter()
                .all(|event| event.visibility == EventVisibility::SeatPrivate(Seat::Two))
        );
        assert_eq!(outcome.replay.lexicon, ruleset.lexicon);
        assert_eq!(outcome.snapshot.state.lexicon, ruleset.lexicon);

        let mut incompatible = ruleset.lexicon.clone();
        incompatible.pack_version = "999.0.0".to_owned();
        let bad_validator: Arc<dyn WordValidator> = Arc::new(FixtureLexicon {
            identity: incompatible,
            words: BTreeSet::new(),
            accept_all: true,
        });
        assert!(matches!(
            Game::create(
                "incompatible-pack",
                ruleset.clone(),
                Some(bad_validator),
                numbered_seed(1),
            ),
            Err(GameError::IncompatibleLexicon(_))
        ));
    }
}

#[test]
fn all_terminal_reasons_are_reproducible() {
    for ruleset in [Ruleset::english_v1(), Ruleset::french_v1()] {
        let rack_out = run_fixture_match(
            &ruleset,
            &[],
            true,
            [BotStrategy::Greedy, BotStrategy::Greedy],
            211,
            128,
        )
        .unwrap();
        assert!(matches!(
            rack_out.result.reason,
            TerminalReason::RackEmptied { .. }
        ));

        let scoreless = run_fixture_match(
            &ruleset,
            &[],
            false,
            [BotStrategy::Greedy, BotStrategy::Greedy],
            223,
            16,
        )
        .unwrap();
        assert_eq!(scoreless.result.reason, TerminalReason::ScorelessTurns);

        let lexicon = fixture_validator(&ruleset, &[], true);
        let mut resigned = Game::create(
            "resignation-terminal",
            ruleset,
            Some(Arc::clone(&lexicon)),
            numbered_seed(227),
        )
        .unwrap();
        resigned.apply_move(Seat::One, 0, Move::Resign).unwrap();
        assert_eq!(
            resigned.result().unwrap().reason,
            TerminalReason::Resignation {
                resigned: Seat::One
            }
        );
        let replay = resigned.replay_bundle().unwrap();
        assert_eq!(
            Game::replay(&replay, Some(lexicon)).unwrap().snapshot(),
            resigned.snapshot()
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    #[test]
    fn generated_state_machine_matches_are_deterministic(
        seed in any::<u64>(),
        french in any::<bool>(),
        greedy_first in any::<bool>(),
    ) {
        let ruleset = if french {
            Ruleset::for_language(Language::French).unwrap()
        } else {
            Ruleset::for_language(Language::English).unwrap()
        };
        let first_bot = if greedy_first {
            BotStrategy::Greedy
        } else {
            BotStrategy::RandomLegal { seed: seed_bytes(seed ^ 0xA5A5) }
        };
        let bots = [
            first_bot,
            BotStrategy::RandomLegal { seed: seed_bytes(seed ^ 0x5A5A) },
        ];
        let first = run_fixture_match(&ruleset, &[], true, bots, seed, 128).unwrap();
        let second = run_fixture_match(&ruleset, &[], true, bots, seed, 128).unwrap();
        prop_assert_eq!(outcome_bytes(&first), outcome_bytes(&second));
        assert_terminal_invariants(
            &first,
            usize::try_from(ruleset.game.total_tiles()).unwrap(),
        );
    }
}

#[test]
fn deterministic_stress_suite_completes_one_thousand_games() {
    for game_number in 0..1_000_u64 {
        let ruleset = if game_number % 2 == 0 {
            Ruleset::english_v1()
        } else {
            Ruleset::french_v1()
        };
        let broad_probe = game_number % 100 == 0;
        let bots = if game_number % 3 == 0 {
            [
                BotStrategy::Greedy,
                BotStrategy::RandomLegal {
                    seed: seed_bytes(game_number ^ 0x11),
                },
            ]
        } else {
            [
                BotStrategy::RandomLegal {
                    seed: seed_bytes(game_number ^ 31),
                },
                BotStrategy::RandomLegal {
                    seed: seed_bytes(game_number ^ 0x2f),
                },
            ]
        };
        let outcome = run_fixture_match(&ruleset, &[], broad_probe, bots, game_number + 1, 128)
            .unwrap_or_else(|error| panic!("generated game {game_number} failed: {error}"));
        assert_terminal_invariants(
            &outcome,
            usize::try_from(ruleset.game.total_tiles()).unwrap(),
        );
    }
}

fn run_fixture_match(
    ruleset: &Ruleset,
    words: &[&str],
    accept_all: bool,
    bots: [BotStrategy; 2],
    seed: u64,
    max_turns: u64,
) -> Result<MatchOutcome, MatchError> {
    let generator = MoveGenerator::new(ruleset, words.iter().copied())?;
    let generator = if accept_all {
        generator.with_rack_probes()
    } else {
        generator
    };
    run_match(MatchSpec {
        game_id: format!("{}-{seed}", ruleset.lexicon.locale),
        ruleset: ruleset.clone(),
        lexicon: fixture_validator(ruleset, words, accept_all),
        seed: numbered_seed(seed),
        generator,
        bots,
        max_turns,
    })
}

fn fixture_validator(
    ruleset: &Ruleset,
    words: &[&str],
    accept_all: bool,
) -> Arc<dyn WordValidator> {
    let words = words
        .iter()
        .map(|word| {
            normalize_key(&ruleset.lexicon.normalization.profile, word)
                .unwrap()
                .into_string()
        })
        .collect();
    Arc::new(FixtureLexicon {
        identity: ruleset.lexicon.clone(),
        words,
        accept_all,
    })
}

fn numbered_seed(number: u64) -> GameSeed {
    GameSeed::from_bytes(seed_bytes(number))
}

fn seed_bytes(number: u64) -> [u8; 32] {
    let mut seed = [0_u8; 32];
    for (index, chunk) in seed.chunks_exact_mut(8).enumerate() {
        chunk.copy_from_slice(&number.wrapping_add(index as u64).to_be_bytes());
    }
    seed
}

fn outcome_bytes(outcome: &MatchOutcome) -> Vec<u8> {
    serde_json::to_vec(&(
        outcome.turns,
        &outcome.result,
        &outcome.snapshot,
        &outcome.replay,
        &outcome.public,
        &outcome.seats,
        &outcome.spectator,
    ))
    .unwrap()
}

fn assert_terminal_invariants(outcome: &MatchOutcome, tile_count: usize) {
    assert_eq!(outcome.snapshot.state.phase, GamePhase::Finished);
    assert_eq!(
        outcome.snapshot.state.result.as_ref(),
        Some(&outcome.result)
    );
    assert_eq!(outcome.turns, outcome.snapshot.state.version);
    assert_eq!(
        outcome.public.events.len(),
        usize::try_from(outcome.turns).unwrap() + 1
    );
    assert!(
        outcome
            .public
            .events
            .iter()
            .enumerate()
            .all(|(sequence, event)| event.sequence == sequence as u64)
    );
    assert!(
        outcome
            .snapshot
            .private_events
            .iter()
            .all(|event| event.sequence > 0 && event.sequence <= outcome.turns)
    );
    let board_count = outcome
        .snapshot
        .state
        .board
        .iter()
        .filter(|tile| tile.is_some())
        .count();
    assert_eq!(
        board_count
            + usize::from(outcome.snapshot.state.bag_count)
            + outcome.snapshot.racks[0].len()
            + outcome.snapshot.racks[1].len(),
        tile_count
    );
    assert_eq!(outcome.result.scores, outcome.snapshot.state.scores);
    assert!(matches!(
        outcome.public.events.last().map(|event| &event.kind),
        Some(
            GameEventKind::MovePlayed {
                result: Some(_),
                ..
            } | GameEventKind::Passed {
                result: Some(_),
                ..
            } | GameEventKind::Exchanged {
                result: Some(_),
                ..
            } | GameEventKind::Resigned { .. }
        )
    ));
}

fn contains_key(value: &Value, searched: &str) -> bool {
    match value {
        Value::Object(object) => {
            object.contains_key(searched)
                || object.values().any(|value| contains_key(value, searched))
        }
        Value::Array(values) => values.iter().any(|value| contains_key(value, searched)),
        _ => false,
    }
}
