use std::sync::Arc;

use tempfile::TempDir;
use word_arena_application::{
    MatchStatisticsInput, NormalizedRunStatistics, STATISTICS_SCHEMA_VERSION, SourcedStatistic,
    StatisticsFilter, StatisticsParticipant, StatisticsRecordResult, StatisticsRepository,
    StatisticsRepositoryError, UnixMillis, aggregate_statistics,
};
use word_arena_engine::{Game, GameSeed, Move, Ruleset, Seat, WordValidator};
use word_arena_lexicon::{NormalizedKey, PackIdentity};
use word_arena_persistence::{SqliteStatisticsRepository, connect_and_migrate};

#[tokio::test]
async fn record_is_restart_safe_idempotent_and_scope_filterable() {
    let database = Database::open("record").await;
    let source = source("source-en", Ruleset::english_v1(), 1_000);
    let derived = source.derive().unwrap();
    assert_eq!(
        database
            .repository
            .record(source.clone(), UnixMillis(2_000))
            .await
            .unwrap(),
        StatisticsRecordResult::Applied(derived.clone())
    );
    let restarted = SqliteStatisticsRepository::new(database.pool.clone());
    assert_eq!(
        restarted
            .record(source.clone(), UnixMillis(3_000))
            .await
            .unwrap(),
        StatisticsRecordResult::AlreadyApplied(derived)
    );
    let public = restarted
        .rebuild_public(StatisticsFilter {
            agent_manifest_sha256: Some("a".repeat(64)),
            seat_number: Some(1),
            ..StatisticsFilter::default()
        })
        .await
        .unwrap();
    assert_eq!(public.games, 1);
    assert_eq!(public.losses, 1);
    assert_eq!(public.tool_calls, SourcedStatistic::exact(2));

    let mut changed = source;
    changed.invalid_attempts[0] = 3;
    assert_eq!(
        restarted.record(changed, UnixMillis(3_000)).await,
        Err(StatisticsRepositoryError::Conflict)
    );
}

#[tokio::test]
async fn full_rebuild_matches_pure_aggregate_across_languages_and_dates() {
    let database = Database::open("rebuild").await;
    let english = source("source-en", Ruleset::english_v1(), 1_000);
    let french = source("source-fr", Ruleset::french_v1(), 2_000);
    let expected = aggregate_statistics(
        StatisticsFilter::default(),
        english
            .derive()
            .unwrap()
            .into_iter()
            .chain(french.derive().unwrap()),
    )
    .unwrap();
    database
        .repository
        .record(english, UnixMillis(3_000))
        .await
        .unwrap();
    database
        .repository
        .record(french, UnixMillis(3_001))
        .await
        .unwrap();
    assert_eq!(
        database
            .repository
            .rebuild_operator(StatisticsFilter::default())
            .await
            .unwrap(),
        expected
    );
    let french_only = database
        .repository
        .rebuild_public(StatisticsFilter {
            language: Some("fr".to_owned()),
            finished_from_ms: Some(1_500),
            finished_before_ms: Some(2_500),
            ..StatisticsFilter::default()
        })
        .await
        .unwrap();
    assert_eq!(french_only.games, 2);
    let public_json = String::from_utf8(serde_json::to_vec(&french_only).unwrap()).unwrap();
    assert!(!public_json.contains("word_frequencies"));
    assert!(!public_json.contains("transcript"));
}

#[tokio::test]
async fn writes_roll_back_atomically_and_normalized_tampering_fails_audit() {
    let database = Database::open("audit").await;
    sqlx::query(
        "CREATE TRIGGER reject_second_statistics_seat
         BEFORE INSERT ON statistics_observations
         WHEN NEW.seat_number = 2
         BEGIN SELECT RAISE(ABORT, 'forced rollback'); END",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    assert_eq!(
        database
            .repository
            .record(source("rollback", Ruleset::english_v1(), 1), UnixMillis(2))
            .await,
        Err(StatisticsRepositoryError::Unavailable)
    );
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM statistics_sources WHERE source_id = 'rollback'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
    sqlx::query("DROP TRIGGER reject_second_statistics_seat")
        .execute(&database.pool)
        .await
        .unwrap();

    database
        .repository
        .record(
            source("tampered", Ruleset::english_v1(), 10),
            UnixMillis(11),
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE statistics_observations SET entrant_id = 'substituted'
         WHERE observation_id = 'tampered:seat-1'",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    assert_eq!(
        database
            .repository
            .rebuild_public(StatisticsFilter::default())
            .await,
        Err(StatisticsRepositoryError::Corrupt)
    );
}

struct Database {
    _directory: TempDir,
    pool: sqlx::SqlitePool,
    repository: SqliteStatisticsRepository,
}

impl Database {
    async fn open(label: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("{label}.sqlite3"));
        let pool = connect_and_migrate(&format!("sqlite://{}", path.display()))
            .await
            .unwrap();
        let repository = SqliteStatisticsRepository::new(pool.clone());
        Self {
            _directory: directory,
            pool,
            repository,
        }
    }
}

fn source(source_id: &str, ruleset: Ruleset, finished_at_ms: i64) -> MatchStatisticsInput {
    let validator = Arc::new(AcceptAll(ruleset.lexicon.clone()));
    let mut game = Game::create(
        format!("{source_id}-game"),
        ruleset,
        Some(validator),
        GameSeed::from_bytes([9; 32]),
    )
    .unwrap();
    game.apply_move(Seat::One, 0, Move::Resign).unwrap();
    MatchStatisticsInput {
        schema_version: STATISTICS_SCHEMA_VERSION,
        source_id: source_id.to_owned(),
        tournament_id: Some("tournament".to_owned()),
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
        events: game.events().to_vec(),
        invalid_attempts: [2, 0],
        telemetry: [
            NormalizedRunStatistics {
                turn_durations_ms: Some(vec![25]),
                tool_calls: SourcedStatistic::exact(2),
                input_tokens: SourcedStatistic::estimated(10),
                output_tokens: SourcedStatistic::exact(4),
                cost_microusd: SourcedStatistic::estimated(3),
            },
            NormalizedRunStatistics::unavailable(),
        ],
    }
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
