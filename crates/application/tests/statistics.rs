use std::sync::Arc;

use word_arena_application::{
    MatchStatisticsInput, NormalizedRunStatistics, STATISTICS_SCHEMA_VERSION, SourcedStatistic,
    StatisticAvailability, StatisticsAccumulator, StatisticsError, StatisticsFilter,
    StatisticsParticipant, UnixMillis, aggregate_statistics,
};
use word_arena_engine::{
    Coordinate, EventVisibility, FormedWord, Game, GameEvent, GameEventKind, GameResult, GameSeed,
    Placement, Ruleset, Score, Seat, TerminalReason, Tile, TileId, WordValidator,
};
use word_arena_lexicon::{NormalizedKey, PackIdentity};

#[test]
fn fixture_derives_gameplay_telemetry_and_private_operator_words() {
    let input = fixture("source-1", 1_000, Some(Seat::One));
    let [one, two] = input.derive().unwrap();
    assert_eq!((one.wins, one.losses, one.ties), (1, 0, 0));
    assert_eq!((two.wins, two.losses, two.ties), (0, 1, 0));
    assert_eq!((one.score_for, one.score_against, one.spread), (80, 40, 40));
    assert_eq!((one.scoring_moves, one.move_score, one.bingos), (1, 80, 1));
    assert_eq!((one.invalid_actions, one.passes, one.exchanges), (2, 0, 0));
    assert_eq!(two.passes, 1);
    assert_eq!(one.premium_use.double_word, 1);
    assert_eq!(one.word_frequencies["ETE"], 1);
    assert_eq!(one.turn_latency_total_ms, SourcedStatistic::exact(40));

    let aggregate = aggregate_statistics(StatisticsFilter::default(), [one, two]).unwrap();
    assert_eq!(aggregate.public.games, 2);
    assert_eq!(aggregate.public.win_rate_millionths, Some(500_000));
    assert_eq!(aggregate.public.average_move_score_milli, Some(80_000));
    assert_eq!(aggregate.public.vocabulary_size, 1);
    assert_eq!(aggregate.word_frequencies["ETE"], 1);
    let public_json = String::from_utf8(serde_json::to_vec(&aggregate.public).unwrap()).unwrap();
    assert!(!public_json.contains("ETE"));
    assert!(!public_json.contains("word_frequencies"));
    assert!(!public_json.contains("rack"));
    assert!(!public_json.contains("transcript"));
}

#[test]
fn incremental_and_full_rebuild_match_with_deduplication_and_scopes() {
    let first = fixture("source-1", 1_000, Some(Seat::One))
        .derive()
        .unwrap();
    let second = fixture("source-2", 2_000, None).derive().unwrap();
    let filter = StatisticsFilter {
        language: Some("en".to_owned()),
        agent_manifest_sha256: Some("a".repeat(64)),
        seat_number: Some(1),
        finished_from_ms: Some(500),
        finished_before_ms: Some(3_000),
        ..StatisticsFilter::default()
    };
    let all = first
        .clone()
        .into_iter()
        .chain(second.clone())
        .collect::<Vec<_>>();
    let full = aggregate_statistics(filter.clone(), all.clone()).unwrap();
    let mut incremental = StatisticsAccumulator::new(filter).unwrap();
    for observation in all {
        incremental.add(observation.clone()).unwrap();
        incremental.add(observation).unwrap();
    }
    assert_eq!(incremental.operator().unwrap(), full);
    assert_eq!(full.public.games, 2);
    assert_eq!(full.public.ties, 1);
    assert_eq!(full.source_ids, ["source-1:seat-1", "source-2:seat-1"]);

    let mut changed = first[0].clone();
    changed.invalid_actions += 1;
    assert_eq!(incremental.add(changed), Err(StatisticsError::Conflict));
}

#[test]
fn missing_estimated_and_overflow_states_are_explicit() {
    let observations = fixture("source-1", 1_000, Some(Seat::One))
        .derive()
        .unwrap();
    let aggregate =
        aggregate_statistics(StatisticsFilter::default(), observations.clone()).unwrap();
    assert_eq!(
        aggregate.public.input_tokens.availability,
        StatisticAvailability::Unavailable
    );
    assert_eq!(
        aggregate.public.cost_microusd.availability,
        StatisticAvailability::Unavailable
    );

    let mut accumulator = StatisticsAccumulator::new(StatisticsFilter::default()).unwrap();
    let mut maximum = observations[0].clone();
    maximum.source_id = "overflow-one".to_owned();
    maximum.invalid_actions = u64::MAX;
    accumulator.add(maximum.clone()).unwrap();
    maximum.source_id = "overflow-two".to_owned();
    accumulator.add(maximum).unwrap();
    assert_eq!(accumulator.public(), Err(StatisticsError::Overflow));
}

fn fixture(source_id: &str, finished_at_ms: i64, winner: Option<Seat>) -> MatchStatisticsInput {
    let ruleset = Ruleset::english_v1();
    let validator = Arc::new(AcceptAll(ruleset.lexicon.clone()));
    let game = Game::create(
        format!("{source_id}-game"),
        ruleset.clone(),
        Some(validator),
        GameSeed::from_bytes([7; 32]),
    )
    .unwrap();
    let mut events = game.events().to_vec();
    let scores = match winner {
        Some(Seat::One) => [Score::new(80), Score::new(40)],
        Some(Seat::Two) => [Score::new(40), Score::new(80)],
        None => [Score::new(60), Score::new(60)],
    };
    let result = GameResult {
        game_id: format!("{source_id}-game"),
        ruleset_id: ruleset.id,
        lexicon: ruleset.lexicon.clone(),
        scores,
        winner,
        final_version: 2,
        reason: TerminalReason::ScorelessTurns,
    };
    events.push(GameEvent {
        sequence: 1,
        visibility: EventVisibility::Public,
        lexicon: ruleset.lexicon.clone(),
        kind: GameEventKind::MovePlayed {
            player: Seat::One,
            placements: vec![Placement::new(
                TileId(1),
                Coordinate::new(7, 7),
                Tile::letter("E"),
            )],
            words: vec![FormedWord {
                text: "ETE".to_owned(),
                normalized: "ETE".to_owned(),
                coordinates: vec![
                    Coordinate::new(7, 7),
                    Coordinate::new(7, 8),
                    Coordinate::new(7, 9),
                ],
                letter_score: 15,
                word_multiplier: 2,
                score: 30,
            }],
            bingo_bonus: 50,
            score: 80,
            draw_count: 1,
            rack_counts_after: [7, 7],
            bag_count_after: 85,
            scores_after: [Score::new(80), Score::new(0)],
            scoreless_turns_after: 0,
            next_player: Seat::Two,
            result: None,
        },
    });
    events.push(GameEvent {
        sequence: 2,
        visibility: EventVisibility::Public,
        lexicon: ruleset.lexicon.clone(),
        kind: GameEventKind::Passed {
            player: Seat::Two,
            scoreless_turns_after: 1,
            next_player: Seat::One,
            result: Some(result),
        },
    });
    MatchStatisticsInput {
        schema_version: STATISTICS_SCHEMA_VERSION,
        source_id: source_id.to_owned(),
        tournament_id: Some("tournament-1".to_owned()),
        match_id: format!("{source_id}-match"),
        game_id: format!("{source_id}-game"),
        finished_at: UnixMillis(finished_at_ms),
        participants: [
            StatisticsParticipant {
                entrant_id: "alpha".to_owned(),
                agent_manifest_sha256: Some("a".repeat(64)),
            },
            StatisticsParticipant {
                entrant_id: "beta".to_owned(),
                agent_manifest_sha256: Some("b".repeat(64)),
            },
        ],
        events,
        invalid_attempts: [2, 1],
        telemetry: telemetry(),
    }
}

fn telemetry() -> [NormalizedRunStatistics; 2] {
    [
        NormalizedRunStatistics {
            turn_durations_ms: Some(vec![10, 30]),
            tool_calls: SourcedStatistic::exact(3),
            input_tokens: SourcedStatistic::estimated(100),
            output_tokens: SourcedStatistic::exact(20),
            cost_microusd: SourcedStatistic::estimated(9),
        },
        NormalizedRunStatistics::unavailable(),
    ]
}

#[derive(Debug)]
struct AcceptAll(PackIdentity);

impl WordValidator for AcceptAll {
    fn identity(&self) -> &PackIdentity {
        &self.0
    }

    fn contains(&self, _key: &NormalizedKey) -> bool {
        true
    }
}
