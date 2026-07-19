use std::sync::Arc;

use sha2::{Digest, Sha256};
use word_arena_application::{
    ANALYTICS_EXPORT_SCHEMA_VERSION, ExportAudience, ExportEnvelope, ExportError, ExportProvenance,
    ExportRecord, JsonlExporter, OperatorAnalyticsExport, OperatorReplayExport,
    PublicAnalyticsExport, PublicReplayExport, RATING_EXPORT_SCHEMA_VERSION, RatingExport,
    RatingPool, RatingRowExport, STANDINGS_EXPORT_SCHEMA_VERSION, StandingRowExport,
    StandingsExport, StatisticsFilter, TOURNAMENT_RESULT_EXPORT_SCHEMA_VERSION,
    TournamentMatchExport, TournamentResultExport, UnixMillis, aggregate_statistics,
};
use word_arena_engine::{Game, GameSeed, Move, Ruleset, Seat, WordValidator};
use word_arena_lexicon::{NormalizedKey, PackIdentity};

#[test]
fn every_public_schema_is_checksummed_and_complete_replay_stays_operator_only() {
    let (game, replay, validator) = finished_game();
    let public_replay = PublicReplayExport::from_complete(&replay).unwrap();
    let public_envelope = ExportEnvelope::new(
        ExportRecord::PublicReplay(public_replay.clone()),
        provenance(&["game-1"]),
        ExportAudience::Public,
    )
    .unwrap();
    public_envelope.verify().unwrap();
    let public_json = String::from_utf8(serde_json::to_vec(&public_envelope).unwrap()).unwrap();
    for forbidden in [
        "\"private_events\":",
        "\"rack_after\":",
        "\"tool_arguments\":",
        "\"transcript\":",
        "wa_cap_v1_secret",
    ] {
        assert!(!public_json.contains(forbidden));
    }
    let reproduced = Game::replay_public(&public_replay.replay, Some(validator)).unwrap();
    assert_eq!(reproduced.public_projection(), game.public_projection());

    let operator = OperatorReplayExport::from_complete(replay).unwrap();
    assert_eq!(
        ExportEnvelope::new(
            ExportRecord::OperatorReplay(operator.clone()),
            provenance(&["game-1"]),
            ExportAudience::Public,
        ),
        Err(ExportError::PrivacyViolation)
    );
    let operator_envelope = ExportEnvelope::new(
        ExportRecord::OperatorReplay(operator),
        provenance(&["game-1"]),
        ExportAudience::Operator,
    )
    .unwrap();
    assert!(
        serde_json::to_string(&operator_envelope)
            .unwrap()
            .contains("private_events")
    );
    assert_eq!(operator_envelope.policy.audience, ExportAudience::Operator);
    assert!(!operator_envelope.policy.redacted);
}

#[test]
fn tournament_standings_ratings_and_analytics_have_stable_schemas() {
    let (game, replay, _) = finished_game();
    let replay_envelope = ExportEnvelope::new(
        ExportRecord::PublicReplay(PublicReplayExport::from_complete(&replay).unwrap()),
        provenance(&["game-1"]),
        ExportAudience::Public,
    )
    .unwrap();
    let tournament = TournamentResultExport {
        schema_version: TOURNAMENT_RESULT_EXPORT_SCHEMA_VERSION,
        tournament_id: "paired-1".to_owned(),
        format_identity_sha256: "b".repeat(64),
        matches: vec![TournamentMatchExport {
            sequence: 0,
            match_id: "match-1".to_owned(),
            series_id: "series-1".to_owned(),
            series_game_number: 1,
            seat_one_entrant_id: "alpha".to_owned(),
            seat_two_entrant_id: "beta".to_owned(),
            result: game.result().unwrap(),
            public_replay_sha256: replay_envelope.content_sha256,
        }],
    };
    let standings = standings("paired-1");
    let ratings = RatingExport {
        schema_version: RATING_EXPORT_SCHEMA_VERSION,
        period_sequence: 7,
        pool: rating_pool(),
        rows: vec![
            RatingRowExport {
                entrant_id: "alpha".to_owned(),
                value: word_arena_application::RatingValue::from_f64(1_550.0, 80.0, 0.06).unwrap(),
            },
            RatingRowExport {
                entrant_id: "beta".to_owned(),
                value: word_arena_application::RatingValue::from_f64(1_450.0, 90.0, 0.06).unwrap(),
            },
        ],
    };
    let statistics = aggregate_statistics(StatisticsFilter::default(), []).unwrap();
    let records = [
        ExportRecord::TournamentResult(tournament),
        ExportRecord::Standings(standings),
        ExportRecord::Ratings(ratings),
        ExportRecord::PublicAnalytics(PublicAnalyticsExport {
            schema_version: ANALYTICS_EXPORT_SCHEMA_VERSION,
            statistics: statistics.public.clone(),
        }),
    ];
    for record in records {
        ExportEnvelope::new(record, provenance(&["source-1"]), ExportAudience::Public)
            .unwrap()
            .verify()
            .unwrap();
    }
    ExportEnvelope::new(
        ExportRecord::OperatorAnalytics(OperatorAnalyticsExport {
            schema_version: ANALYTICS_EXPORT_SCHEMA_VERSION,
            statistics,
        }),
        provenance(&["source-1"]),
        ExportAudience::Operator,
    )
    .unwrap();
}

#[test]
fn golden_jsonl_checksum_and_mutation_detection_are_deterministic() {
    let envelope = ExportEnvelope::new(
        ExportRecord::Standings(standings("golden")),
        provenance(&["match-1", "match-2"]),
        ExportAudience::Public,
    )
    .unwrap();
    let mut exporter = JsonlExporter::new(Vec::new(), 16_384, 32_768).unwrap();
    exporter.write(&envelope).unwrap();
    let (bytes, summary) = exporter.finish().unwrap();
    assert_eq!(
        String::from_utf8(bytes.clone()).unwrap(),
        include_str!("snapshots/export.jsonl")
    );
    assert_eq!(summary.record_count, 1);
    assert_eq!(summary.byte_count, u64::try_from(bytes.len()).unwrap());
    assert_eq!(summary.checksum_sha256, digest(&bytes));

    let mut changed = envelope;
    let ExportRecord::Standings(export) = &mut changed.record else {
        unreachable!();
    };
    export.rows[0].spread += 1;
    assert_eq!(changed.verify(), Err(ExportError::ChecksumMismatch));
}

#[test]
fn large_stream_is_bounded_ordered_and_byte_deterministic() {
    let first = large_stream().unwrap();
    let second = large_stream().unwrap();
    assert_eq!(first, second);
    assert_eq!(first.1.record_count, 2_000);
    assert_eq!(first.1.checksum_sha256, digest(&first.0));

    let later = ExportEnvelope::new(
        ExportRecord::Standings(standings("z")),
        provenance(&["z"]),
        ExportAudience::Public,
    )
    .unwrap();
    let earlier = ExportEnvelope::new(
        ExportRecord::Standings(standings("a")),
        provenance(&["a"]),
        ExportAudience::Public,
    )
    .unwrap();
    let mut exporter = JsonlExporter::new(Vec::new(), 16_384, 32_768).unwrap();
    exporter.write(&later).unwrap();
    assert_eq!(exporter.write(&earlier), Err(ExportError::OutOfOrder));

    let mut tiny = JsonlExporter::new(Vec::new(), 16, 32_768).unwrap();
    assert_eq!(tiny.write(&earlier), Err(ExportError::RecordTooLarge));
}

#[test]
fn credential_shaped_content_and_unknown_schemas_fail_closed() {
    let mut unsafe_provenance = provenance(&["source"]);
    unsafe_provenance.producer = "Bearer wa_cap_v1_secret".to_owned();
    assert_eq!(
        ExportEnvelope::new(
            ExportRecord::Standings(standings("safe")),
            unsafe_provenance,
            ExportAudience::Public,
        ),
        Err(ExportError::InvalidInput)
    );
    let envelope = ExportEnvelope::new(
        ExportRecord::Standings(standings("safe")),
        provenance(&["source"]),
        ExportAudience::Public,
    )
    .unwrap();
    let mut value = serde_json::to_value(envelope).unwrap();
    value["schema_version"] = serde_json::json!(99);
    let incompatible: ExportEnvelope = serde_json::from_value(value).unwrap();
    assert_eq!(incompatible.verify(), Err(ExportError::IncompatibleSchema));
}

fn large_stream() -> Result<(Vec<u8>, word_arena_application::ExportSummary), ExportError> {
    let mut exporter = JsonlExporter::new(Vec::new(), 16_384, 8 * 1024 * 1024)?;
    for index in 0..2_000 {
        let tournament_id = format!("tournament-{index:04}");
        let envelope = ExportEnvelope::new(
            ExportRecord::Standings(standings(&tournament_id)),
            provenance(&[&format!("source-{index:04}")]),
            ExportAudience::Public,
        )?;
        exporter.write(&envelope)?;
    }
    exporter.finish()
}

fn standings(tournament_id: &str) -> StandingsExport {
    StandingsExport {
        schema_version: STANDINGS_EXPORT_SCHEMA_VERSION,
        tournament_id: tournament_id.to_owned(),
        rows: vec![
            StandingRowExport {
                rank: 1,
                entrant_id: "alpha".to_owned(),
                played: 2,
                wins: 2,
                losses: 0,
                ties: 0,
                spread: 42,
            },
            StandingRowExport {
                rank: 2,
                entrant_id: "beta".to_owned(),
                played: 2,
                wins: 0,
                losses: 2,
                ties: 0,
                spread: -42,
            },
        ],
    }
}

fn provenance(source_ids: &[&str]) -> ExportProvenance {
    ExportProvenance {
        producer: "word-arena-test".to_owned(),
        generated_at: UnixMillis(1_234),
        source_ids: source_ids.iter().map(|value| (*value).to_owned()).collect(),
        source_sha256s: vec!["a".repeat(64)],
    }
}

fn rating_pool() -> RatingPool {
    RatingPool {
        language: "en".to_owned(),
        ruleset_id: "english-v1".to_owned(),
        ruleset_sha256: "c".repeat(64),
        rated_format_policy: "paired-v1".to_owned(),
    }
}

fn finished_game() -> (Game, word_arena_engine::ReplayBundle, Arc<AcceptAll>) {
    let ruleset = Ruleset::english_v1();
    let validator = Arc::new(AcceptAll(ruleset.lexicon.clone()));
    let mut game = Game::create(
        "game-1",
        ruleset,
        Some(validator.clone()),
        GameSeed::from_bytes([31; 32]),
    )
    .unwrap();
    let tile_id = game.rack(Seat::One).tiles()[0].id;
    game.apply_move(
        Seat::One,
        0,
        Move::Exchange {
            tile_ids: vec![tile_id],
        },
    )
    .unwrap();
    game.apply_move(Seat::Two, 1, Move::Resign).unwrap();
    let replay = game.replay_bundle().unwrap();
    (game, replay, validator)
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut output, byte| {
            use std::fmt::Write;
            write!(&mut output, "{byte:02x}").unwrap();
            output
        })
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
