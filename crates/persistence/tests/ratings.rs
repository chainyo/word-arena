use tempfile::TempDir;
use word_arena_application::{
    RATING_SCHEMA_VERSION, RatedMatchInput, RatingCommitResult, RatingOpponent, RatingPeriod,
    RatingPool, RatingRepository, RatingRepositoryError, RatingUpdateInput, RatingValue,
    SCORE_SCALE, UnixMillis,
};
use word_arena_persistence::{SqliteRatingRepository, connect_and_migrate};

#[tokio::test]
async fn paired_games_round_trip_once_and_commit_is_idempotent() {
    let database = Database::open("paired").await;
    let period = paired_period("period-0", 0, pool("en", 'a'), initial(), 1_000_000);
    let expected = period.derive().unwrap();

    assert_eq!(
        database
            .repository
            .commit(period.clone(), UnixMillis(10))
            .await
            .unwrap(),
        RatingCommitResult::Applied(word_arena_application::StoredRatingPeriod {
            period: period.clone(),
            derived: expected.clone(),
            committed_at: UnixMillis(10),
        })
    );
    let restarted = SqliteRatingRepository::new(database.pool.clone());
    assert_eq!(restarted.load("period-0").await.unwrap().derived, expected);
    assert!(matches!(
        restarted
            .commit(period.clone(), UnixMillis(99))
            .await
            .unwrap(),
        RatingCommitResult::AlreadyApplied(_)
    ));
    let match_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM rating_match_inputs WHERE period_id = 'period-0'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(match_count, 2);

    let mut conflict = period;
    conflict.matches[0].series_id = "changed".to_owned();
    assert_eq!(
        restarted.commit(conflict, UnixMillis(100)).await,
        Err(RatingRepositoryError::Conflict)
    );
}

#[tokio::test]
async fn rebuild_is_deterministic_and_pools_are_isolated() {
    let database = Database::open("rebuild").await;
    let english = pool("en", 'a');
    let french = pool("fr", 'b');
    let first = paired_period("en-0", 0, english.clone(), initial(), SCORE_SCALE);
    let first_derived = first.derive().unwrap();
    database
        .repository
        .commit(first, UnixMillis(1))
        .await
        .unwrap();
    let second = paired_period(
        "en-1",
        1,
        english.clone(),
        (&first_derived[0].1, &first_derived[1].1),
        SCORE_SCALE / 2,
    );
    let expected = second.derive().unwrap();
    database
        .repository
        .commit(second, UnixMillis(2))
        .await
        .unwrap();
    let french_period = paired_period("fr-0", 0, french.clone(), initial(), 0);
    let french_expected = french_period.derive().unwrap();
    database
        .repository
        .commit(french_period, UnixMillis(3))
        .await
        .unwrap();

    assert_eq!(
        database.repository.rebuild(english.clone()).await.unwrap(),
        expected
    );
    assert_eq!(
        database.repository.rebuild(english).await.unwrap(),
        expected
    );
    assert_eq!(
        database.repository.rebuild(french).await.unwrap(),
        french_expected
    );
}

#[tokio::test]
async fn conflicting_match_rolls_back_and_normalized_tampering_is_detected() {
    let database = Database::open("audit").await;
    let identity = pool("en", 'c');
    let first = paired_period("audit-0", 0, identity.clone(), initial(), SCORE_SCALE);
    let derived = first.derive().unwrap();
    database
        .repository
        .commit(first.clone(), UnixMillis(1))
        .await
        .unwrap();

    let mut duplicate = paired_period(
        "audit-1",
        1,
        identity.clone(),
        (&derived[0].1, &derived[1].1),
        0,
    );
    duplicate.matches[0].match_id = first.matches[0].match_id.clone();
    assert_eq!(
        database.repository.commit(duplicate, UnixMillis(2)).await,
        Err(RatingRepositoryError::Conflict)
    );
    let rolled_back = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM rating_periods WHERE period_id = 'audit-1'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(rolled_back, 0);

    sqlx::query(
        "UPDATE rating_match_inputs SET score_one_millionths = 500000
         WHERE period_id = 'audit-0' AND series_game_number = 1",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    assert_eq!(
        database.repository.load("audit-0").await,
        Err(RatingRepositoryError::Corrupt)
    );
    assert_eq!(
        database.repository.rebuild(identity).await,
        Err(RatingRepositoryError::Corrupt)
    );
}

struct Database {
    _directory: TempDir,
    pool: sqlx::SqlitePool,
    repository: SqliteRatingRepository,
}

impl Database {
    async fn open(label: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("{label}.sqlite3"));
        let pool = connect_and_migrate(&format!("sqlite://{}", path.display()))
            .await
            .unwrap();
        let repository = SqliteRatingRepository::new(pool.clone());
        Self {
            _directory: directory,
            pool,
            repository,
        }
    }
}

fn initial() -> (&'static RatingValue, &'static RatingValue) {
    static INITIAL: std::sync::LazyLock<RatingValue> =
        std::sync::LazyLock::new(|| RatingValue::from_f64(1_500.0, 350.0, 0.06).unwrap());
    (&INITIAL, &INITIAL)
}

fn pool(language: &str, marker: char) -> RatingPool {
    RatingPool {
        language: language.to_owned(),
        ruleset_id: "classic-v1".to_owned(),
        ruleset_sha256: marker.to_string().repeat(64),
        rated_format_policy: "paired-seat-swap-v1".to_owned(),
    }
}

fn paired_period(
    period_id: &str,
    sequence: u64,
    pool: RatingPool,
    previous: (&RatingValue, &RatingValue),
    score_one_millionths: u32,
) -> RatingPeriod {
    let reverse_score = SCORE_SCALE - score_one_millionths;
    RatingPeriod {
        schema_version: RATING_SCHEMA_VERSION,
        period_id: period_id.to_owned(),
        sequence,
        pool,
        matches: vec![
            RatedMatchInput {
                match_id: format!("{period_id}-game-1"),
                series_id: format!("{period_id}-series"),
                series_game_number: 1,
                entrant_one: "alpha".to_owned(),
                entrant_two: "beta".to_owned(),
                score_one_millionths,
            },
            RatedMatchInput {
                match_id: format!("{period_id}-game-2"),
                series_id: format!("{period_id}-series"),
                series_game_number: 2,
                entrant_one: "beta".to_owned(),
                entrant_two: "alpha".to_owned(),
                score_one_millionths: reverse_score,
            },
        ],
        updates: vec![
            RatingUpdateInput {
                entrant_id: "alpha".to_owned(),
                previous: *previous.0,
                opponents: vec![
                    RatingOpponent {
                        rating: *previous.1,
                        score_millionths: score_one_millionths,
                    },
                    RatingOpponent {
                        rating: *previous.1,
                        score_millionths: score_one_millionths,
                    },
                ],
            },
            RatingUpdateInput {
                entrant_id: "beta".to_owned(),
                previous: *previous.1,
                opponents: vec![
                    RatingOpponent {
                        rating: *previous.0,
                        score_millionths: reverse_score,
                    },
                    RatingOpponent {
                        rating: *previous.0,
                        score_millionths: reverse_score,
                    },
                ],
            },
        ],
    }
}
