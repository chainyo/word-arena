use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;
use word_arena_application::{
    EntrantPairing, SeatBalance, SeriesSeatPolicy, SwissProgress, SwissRematchPolicy,
    SwissStanding, TOURNAMENT_FORMAT_SCHEMA_VERSION, TOURNAMENT_SCHEDULE_SCHEMA_VERSION,
    TournamentEntrant, TournamentError, TournamentFormat, TournamentGameProfile, TournamentSpec,
};
use word_arena_lexicon::{NormalizationDescriptor, PackIdentity};

fn profile(language: &str, marker: char) -> TournamentGameProfile {
    TournamentGameProfile {
        language: language.to_owned(),
        ruleset_id: format!("{language}-v1"),
        ruleset_sha256: marker.to_string().repeat(64),
        lexicon: PackIdentity {
            pack_id: format!("word-arena-{language}-v1"),
            pack_version: "1.0.0".to_owned(),
            format_version: 1,
            locale: language.to_owned(),
            normalization: NormalizationDescriptor {
                algorithm: "word-arena-board-key".to_owned(),
                version: 1,
                profile: format!("{language}-v1"),
            },
            content_sha256: marker.to_string().repeat(64),
        },
    }
}

fn entrants(count: usize) -> Vec<TournamentEntrant> {
    (1..=count)
        .map(|seed| TournamentEntrant {
            entrant_id: format!("agent-{seed}"),
            seed_number: u32::try_from(seed).unwrap(),
            manifest_sha256: None,
        })
        .collect()
}

fn commitments(count: usize) -> Vec<String> {
    (1..=count).map(|value| format!("{value:064x}")).collect()
}

fn spec(count: usize, format: TournamentFormat, seed_count: usize) -> TournamentSpec {
    TournamentSpec {
        schema_version: TOURNAMENT_FORMAT_SCHEMA_VERSION,
        tournament_id: "tournament-one".to_owned(),
        format,
        entrants: entrants(count),
        profiles: vec![profile("en", 'a'), profile("fr", 'b')],
        game_seed_commitments: commitments(seed_count),
    }
}

#[test]
fn four_entrant_round_robin_has_a_stable_golden_schedule() {
    let schedule = spec(4, TournamentFormat::RoundRobin { cycles: 1 }, 6)
        .schedule()
        .unwrap();
    assert_eq!(schedule.schema_version, TOURNAMENT_SCHEDULE_SCHEMA_VERSION);
    assert_eq!(schedule.series.len(), 6);
    assert!(schedule.byes.is_empty());
    let golden = schedule
        .matches
        .iter()
        .map(|game| {
            (
                game.round_number,
                game.table_number,
                game.seat_one_entrant_id.as_str(),
                game.seat_two_entrant_id.as_str(),
                game.profile.language.as_str(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        golden,
        [
            (1, 1, "agent-4", "agent-1", "en"),
            (1, 2, "agent-2", "agent-3", "en"),
            (2, 1, "agent-1", "agent-3", "fr"),
            (2, 2, "agent-4", "agent-2", "fr"),
            (3, 1, "agent-1", "agent-2", "en"),
            (3, 2, "agent-3", "agent-4", "en"),
        ]
    );
    assert_eq!(
        serde_json::to_vec(&schedule).unwrap(),
        serde_json::to_vec(
            &spec(4, TournamentFormat::RoundRobin { cycles: 1 }, 6)
                .schedule()
                .unwrap()
        )
        .unwrap()
    );
}

#[test]
fn paired_swap_is_exact_across_seats_languages_and_byes() {
    let schedule = spec(5, TournamentFormat::PairedSeatSwap { cycles: 1 }, 20)
        .schedule()
        .unwrap();
    assert_eq!(schedule.byes.len(), 5);
    assert_eq!(
        schedule
            .byes
            .iter()
            .map(|bye| bye.entrant_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        5
    );
    for series in &schedule.series {
        let games = schedule
            .matches
            .iter()
            .filter(|game| game.series_id == series.series_id)
            .collect::<Vec<_>>();
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].seat_one_entrant_id, games[1].seat_two_entrant_id);
        assert_eq!(games[0].seat_two_entrant_id, games[1].seat_one_entrant_id);
        assert_eq!(games[0].profile.language, games[1].profile.language);
    }
    let balance = seat_counts(&schedule.matches);
    for (one, two) in balance.values() {
        assert_eq!(one, two);
    }
    let mut exposure = BTreeMap::<String, BTreeMap<String, u32>>::new();
    for game in &schedule.matches {
        for entrant in [&game.seat_one_entrant_id, &game.seat_two_entrant_id] {
            *exposure
                .entry(entrant.clone())
                .or_default()
                .entry(game.profile.language.clone())
                .or_default() += 1;
        }
    }
    for languages in exposure.values() {
        assert_eq!(languages.get("en"), languages.get("fr"));
    }
    assert_no_simultaneous_duplicate(&schedule.matches);
}

#[test]
fn configurable_series_and_format_identity_are_versioned_and_strict() {
    let format = TournamentFormat::Series {
        cycles: 1,
        games_per_series: 3,
        seat_policy: SeriesSeatPolicy::Alternate,
    };
    let first = spec(3, format.clone(), 9);
    let schedule = first.schedule().unwrap();
    assert_eq!(schedule.matches.len(), 9);
    for (one, two) in seat_counts(&schedule.matches).values() {
        assert!(one.abs_diff(*two) <= 1);
    }
    assert_no_simultaneous_duplicate(&schedule.matches);
    let mut different_entrants = first.clone();
    different_entrants.entrants.reverse();
    assert_eq!(
        first.format_identity().unwrap(),
        different_entrants.format_identity().unwrap(),
        "entrant ordering is not format identity"
    );
    let mut different_profile = first.clone();
    different_profile.profiles.swap(0, 1);
    assert_ne!(
        first.format_identity().unwrap(),
        different_profile.format_identity().unwrap()
    );

    let invalid = spec(
        3,
        TournamentFormat::Series {
            cycles: 1,
            games_per_series: 3,
            seat_policy: SeriesSeatPolicy::PairedSwap,
        },
        9,
    );
    assert_eq!(invalid.schedule(), Err(TournamentError::InvalidSpec));
    let mut extra = first;
    extra.game_seed_commitments.push("f".repeat(64));
    assert_eq!(extra.schedule(), Err(TournamentError::ExtraGameSeeds));
}

#[test]
fn swiss_progression_uses_standings_avoids_rematches_and_rotates_byes() {
    let tournament = spec(
        5,
        TournamentFormat::Swiss {
            rounds: 3,
            games_per_series: 1,
            rematches: SwissRematchPolicy::Avoid,
        },
        6,
    );
    let standings = entrants(5)
        .into_iter()
        .map(|entrant| SwissStanding {
            entrant_id: entrant.entrant_id,
            match_points: 0,
            spread: 0,
            wins: 0,
        })
        .collect();
    let first_progress = SwissProgress {
        completed_rounds: 0,
        standings,
        prior_pairings: BTreeSet::new(),
        prior_byes: BTreeSet::new(),
        seat_balance: Vec::new(),
        next_seed_index: 0,
        next_match_sequence: 0,
    };
    let first = tournament.schedule_swiss_round(&first_progress).unwrap();
    assert_eq!(first.byes[0].entrant_id, "agent-5");
    assert_eq!(pair_set(&first.matches).len(), 2);

    let prior_pairings = pair_set(&first.matches);
    let second_progress = SwissProgress {
        completed_rounds: 1,
        standings: vec![
            standing("agent-1", 3, 20),
            standing("agent-3", 3, 10),
            standing("agent-5", 1, 0),
            standing("agent-2", 0, -10),
            standing("agent-4", 0, -20),
        ],
        prior_pairings: prior_pairings.clone(),
        prior_byes: BTreeSet::from(["agent-5".to_owned()]),
        seat_balance: first
            .matches
            .iter()
            .fold(BTreeMap::new(), |mut counts, game| {
                *counts
                    .entry(game.seat_one_entrant_id.clone())
                    .or_insert((0, 0)) = (1, 0);
                *counts
                    .entry(game.seat_two_entrant_id.clone())
                    .or_insert((0, 0)) = (0, 1);
                counts
            })
            .into_iter()
            .map(
                |(entrant_id, (seat_one_games, seat_two_games))| SeatBalance {
                    entrant_id,
                    seat_one_games,
                    seat_two_games,
                },
            )
            .collect(),
        next_seed_index: first.matches.len(),
        next_match_sequence: first.matches.len() as u64,
    };
    let second = tournament.schedule_swiss_round(&second_progress).unwrap();
    assert_ne!(second.byes[0].entrant_id, "agent-5");
    assert!(pair_set(&second.matches).is_disjoint(&prior_pairings));
    assert!(second.matches.iter().all(|game| game.round_number == 2));
    assert_eq!(second.matches[0].sequence, 2);
}

#[test]
fn swiss_rematch_policy_fails_closed_or_falls_back_explicitly() {
    let progress = SwissProgress {
        completed_rounds: 1,
        standings: vec![standing("agent-1", 3, 1), standing("agent-2", 0, -1)],
        prior_pairings: BTreeSet::from([EntrantPairing {
            entrant_a: "agent-1".to_owned(),
            entrant_b: "agent-2".to_owned(),
        }]),
        prior_byes: BTreeSet::new(),
        seat_balance: Vec::new(),
        next_seed_index: 1,
        next_match_sequence: 1,
    };
    let avoid = spec(
        2,
        TournamentFormat::Swiss {
            rounds: 2,
            games_per_series: 1,
            rematches: SwissRematchPolicy::Avoid,
        },
        2,
    );
    assert_eq!(
        avoid.schedule_swiss_round(&progress),
        Err(TournamentError::SwissPairingUnavailable)
    );
    let allow = spec(
        2,
        TournamentFormat::Swiss {
            rounds: 2,
            games_per_series: 1,
            rematches: SwissRematchPolicy::AllowWhenRequired,
        },
        2,
    );
    assert_eq!(
        allow.schedule_swiss_round(&progress).unwrap().matches.len(),
        1
    );
}

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn round_robin_pair_coverage_balance_and_concurrency_are_invariant(count in 2_usize..11) {
        let pair_count = count * (count - 1) / 2;
        let schedule = spec(
            count,
            TournamentFormat::RoundRobin { cycles: 1 },
            pair_count,
        )
        .schedule()
        .unwrap();
        prop_assert_eq!(schedule.matches.len(), pair_count);
        prop_assert_eq!(pair_set(&schedule.matches).len(), pair_count);
        for (one, two) in seat_counts(&schedule.matches).values() {
            prop_assert!(one.abs_diff(*two) <= 1);
        }
        assert_no_simultaneous_duplicate(&schedule.matches);
        let replay = spec(
            count,
            TournamentFormat::RoundRobin { cycles: 1 },
            pair_count,
        )
        .schedule()
        .unwrap();
        prop_assert_eq!(serde_json::to_vec(&schedule).unwrap(), serde_json::to_vec(&replay).unwrap());
        if !count.is_multiple_of(2) {
            prop_assert_eq!(schedule.byes.len(), count);
        }
    }
}

fn standing(entrant_id: &str, match_points: i64, spread: i64) -> SwissStanding {
    SwissStanding {
        entrant_id: entrant_id.to_owned(),
        match_points,
        spread,
        wins: u32::from(match_points > 0),
    }
}

fn pair_set(matches: &[word_arena_application::ScheduledMatch]) -> BTreeSet<EntrantPairing> {
    matches
        .iter()
        .map(|game| {
            let (entrant_a, entrant_b) = if game.seat_one_entrant_id <= game.seat_two_entrant_id {
                (
                    game.seat_one_entrant_id.clone(),
                    game.seat_two_entrant_id.clone(),
                )
            } else {
                (
                    game.seat_two_entrant_id.clone(),
                    game.seat_one_entrant_id.clone(),
                )
            };
            EntrantPairing {
                entrant_a,
                entrant_b,
            }
        })
        .collect()
}

fn seat_counts(matches: &[word_arena_application::ScheduledMatch]) -> BTreeMap<String, (u32, u32)> {
    let mut counts = BTreeMap::new();
    for game in matches {
        counts
            .entry(game.seat_one_entrant_id.clone())
            .or_insert((0_u32, 0_u32))
            .0 += 1;
        counts
            .entry(game.seat_two_entrant_id.clone())
            .or_insert((0_u32, 0_u32))
            .1 += 1;
    }
    counts
}

fn assert_no_simultaneous_duplicate(matches: &[word_arena_application::ScheduledMatch]) {
    let mut assignments = BTreeSet::new();
    for game in matches {
        for entrant in [&game.seat_one_entrant_id, &game.seat_two_entrant_id] {
            assert!(assignments.insert((
                game.round_number,
                game.series_game_number,
                entrant.as_str()
            )));
        }
    }
}
