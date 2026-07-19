use tempfile::TempDir;
use word_arena_application::{
    ExecutionReservation, RatePolicy, ReservationRequest, ReservationResult,
    SCHEDULER_SCHEMA_VERSION, SchedulerRepository, SchedulerRepositoryError, SchedulerScope,
    SchedulingLimit, TerminalCommitResult, TerminalMatchResult, TournamentWorkerControl,
    UnixMillis,
};
use word_arena_persistence::{SqliteSchedulerRepository, connect_and_migrate};

#[tokio::test]
async fn four_scopes_enforce_concurrency_and_deterministic_rate_refill() {
    let database = Database::open("limits").await;
    database.configure(1, Some(rate(1, 1, 10)), 0).await;
    let first = acquired(
        database
            .repository
            .acquire(request("one", "match-1", 0, 100))
            .await
            .unwrap(),
    );
    assert_eq!(
        database
            .repository
            .acquire(request("two", "match-2", 1, 100))
            .await
            .unwrap(),
        ReservationResult::Limited { retry_at: None }
    );
    database
        .repository
        .release(first, UnixMillis(2))
        .await
        .unwrap();
    assert_eq!(
        database
            .repository
            .acquire(request("two", "match-2", 2, 100))
            .await
            .unwrap(),
        ReservationResult::Limited {
            retry_at: Some(UnixMillis(10))
        }
    );
    assert!(matches!(
        database
            .repository
            .acquire(request("two", "match-2", 10, 100))
            .await
            .unwrap(),
        ReservationResult::Acquired(_)
    ));
}

#[tokio::test]
async fn pause_drain_cancel_and_restart_reconstruction_are_durable() {
    let database = Database::open("control").await;
    database.configure(4, None, 0).await;
    database
        .repository
        .set_control("tournament", TournamentWorkerControl::Paused, UnixMillis(1))
        .await
        .unwrap();
    assert_eq!(
        database
            .repository
            .acquire(request("one", "match-1", 2, 10))
            .await
            .unwrap(),
        ReservationResult::Paused
    );
    database
        .repository
        .set_control(
            "tournament",
            TournamentWorkerControl::Running,
            UnixMillis(3),
        )
        .await
        .unwrap();
    let active = acquired(
        database
            .repository
            .acquire(request("one", "match-1", 4, 10))
            .await
            .unwrap(),
    );
    database
        .repository
        .set_control(
            "tournament",
            TournamentWorkerControl::Draining,
            UnixMillis(5),
        )
        .await
        .unwrap();
    assert_eq!(
        database
            .repository
            .acquire(request("two", "match-2", 5, 10))
            .await
            .unwrap(),
        ReservationResult::Draining
    );

    let restarted = SqliteSchedulerRepository::new(database.pool.clone());
    let snapshot = restarted.reconstruct(UnixMillis(13)).await.unwrap();
    assert_eq!(snapshot.active.as_slice(), std::slice::from_ref(&active));
    let expired = restarted.reconstruct(UnixMillis(14)).await.unwrap();
    assert_eq!(expired.expired_match_ids, ["match-1"]);
    assert!(expired.active.is_empty());

    restarted
        .set_control(
            "tournament",
            TournamentWorkerControl::Running,
            UnixMillis(15),
        )
        .await
        .unwrap();
    let cancelled = acquired(
        restarted
            .acquire(request("three", "match-3", 16, 10))
            .await
            .unwrap(),
    );
    restarted
        .set_control(
            "tournament",
            TournamentWorkerControl::Cancelled,
            UnixMillis(17),
        )
        .await
        .unwrap();
    assert_eq!(
        restarted.renew(cancelled.clone(), UnixMillis(18), 10).await,
        Err(SchedulerRepositoryError::Conflict)
    );
    restarted.release(cancelled, UnixMillis(18)).await.unwrap();
    assert_eq!(
        restarted
            .acquire(request("four", "match-4", 19, 10))
            .await
            .unwrap(),
        ReservationResult::Cancelled
    );
}

#[tokio::test]
async fn terminal_commit_is_exactly_once_and_fences_downstream_side_effects() {
    let database = Database::open("terminal").await;
    database.configure(2, None, 0).await;
    let reservation = acquired(
        database
            .repository
            .acquire(request("one", "match-1", 1, 100))
            .await
            .unwrap(),
    );
    let result = terminal("match-1", "one");
    assert_eq!(
        database
            .repository
            .commit_terminal(reservation.clone(), result.clone(), UnixMillis(2))
            .await
            .unwrap(),
        TerminalCommitResult::Applied(result.clone())
    );
    assert_eq!(
        database
            .repository
            .commit_terminal(reservation.clone(), result.clone(), UnixMillis(3))
            .await
            .unwrap(),
        TerminalCommitResult::AlreadyApplied(result.clone())
    );
    let mut changed = result.clone();
    changed.result_sha256 = "d".repeat(64);
    assert_eq!(
        database
            .repository
            .commit_terminal(reservation, changed, UnixMillis(3))
            .await,
        Err(SchedulerRepositoryError::Conflict)
    );
    assert_eq!(
        database
            .repository
            .acquire(request("retry", "match-1", 4, 100))
            .await
            .unwrap(),
        ReservationResult::AlreadyFinished
    );

    let other = acquired(
        database
            .repository
            .acquire(request("two", "match-2", 4, 100))
            .await
            .unwrap(),
    );
    let mut duplicate_charge = terminal("match-2", "two");
    duplicate_charge.charge_key = result.charge_key;
    assert_eq!(
        database
            .repository
            .commit_terminal(other, duplicate_charge, UnixMillis(5))
            .await,
        Err(SchedulerRepositoryError::Conflict)
    );
}

#[tokio::test]
async fn concurrent_reservations_respect_limit_and_retry_cannot_change_inputs() {
    let database = Database::open("concurrent").await;
    database.configure(2, None, 0).await;
    let mut tasks = Vec::new();
    for index in 0..8 {
        let repository = database.repository.clone();
        tasks.push(tokio::spawn(async move {
            repository
                .acquire(request(
                    &format!("reservation-{index}"),
                    &format!("match-{index}"),
                    1,
                    10,
                ))
                .await
                .unwrap()
        }));
    }
    let mut acquired_count = 0;
    for task in tasks {
        if matches!(task.await.unwrap(), ReservationResult::Acquired(_)) {
            acquired_count += 1;
        }
    }
    assert_eq!(acquired_count, 2);

    let first = acquired(
        database
            .repository
            .acquire(request("retry-one", "retry-match", 20, 5))
            .await
            .unwrap(),
    );
    database
        .repository
        .reconstruct(UnixMillis(25))
        .await
        .unwrap();
    let mut changed = request("retry-two", "retry-match", 26, 5);
    changed.immutable_inputs_sha256 = "c".repeat(64);
    assert_eq!(
        database.repository.acquire(changed).await,
        Err(SchedulerRepositoryError::Conflict)
    );
    assert!(first.expires_at <= UnixMillis(25));
}

#[tokio::test]
async fn cancellation_and_terminal_commit_race_has_one_durable_winner() {
    let database = Database::open("cancel-race").await;
    database.configure(2, None, 0).await;
    let reservation = acquired(
        database
            .repository
            .acquire(request("reservation", "match-race", 1, 100))
            .await
            .unwrap(),
    );
    let commit_repository = database.repository.clone();
    let cancel_repository = database.repository.clone();
    let (commit, cancel) = tokio::join!(
        commit_repository.commit_terminal(
            reservation,
            terminal("match-race", "race"),
            UnixMillis(2),
        ),
        cancel_repository.set_control(
            "tournament",
            TournamentWorkerControl::Cancelled,
            UnixMillis(2),
        )
    );
    cancel.unwrap();
    assert!(matches!(
        commit,
        Ok(TerminalCommitResult::Applied(_)) | Err(SchedulerRepositoryError::Conflict)
    ));
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM terminal_match_results WHERE match_id = 'match-race'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(count <= 1);
}

#[tokio::test]
async fn fixed_concurrency_tournament_finishes_once_per_scheduled_match() {
    let database = Database::open("fixed-tournament").await;
    database.configure(2, None, 0).await;
    for wave in 0..3 {
        let first_index = wave * 2;
        let second_index = first_index + 1;
        let first_match = format!("match-{first_index}");
        let second_match = format!("match-{second_index}");
        let first = acquired(
            database
                .repository
                .acquire(request(
                    &format!("r-{first_index}"),
                    &first_match,
                    wave + 1,
                    100,
                ))
                .await
                .unwrap(),
        );
        let second = acquired(
            database
                .repository
                .acquire(request(
                    &format!("r-{second_index}"),
                    &second_match,
                    wave + 1,
                    100,
                ))
                .await
                .unwrap(),
        );
        assert!(matches!(
            database
                .repository
                .acquire(request("blocked", "not-scheduled", wave + 1, 100))
                .await
                .unwrap(),
            ReservationResult::Limited { .. }
        ));
        for (reservation, match_id, marker) in [
            (first, first_match, format!("{first_index}")),
            (second, second_match, format!("{second_index}")),
        ] {
            database
                .repository
                .commit_terminal(
                    reservation,
                    terminal(&match_id, &marker),
                    UnixMillis(wave + 2),
                )
                .await
                .unwrap();
        }
    }
    let total = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM terminal_match_results")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    let distinct =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(DISTINCT match_id) FROM terminal_match_results")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!((total, distinct), (6, 6));
}

struct Database {
    _directory: TempDir,
    pool: sqlx::SqlitePool,
    repository: SqliteSchedulerRepository,
}

impl Database {
    async fn open(label: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("{label}.sqlite3"));
        let pool = connect_and_migrate(&format!("sqlite://{}", path.display()))
            .await
            .unwrap();
        Self {
            _directory: directory,
            repository: SqliteSchedulerRepository::new(pool.clone()),
            pool,
        }
    }

    async fn configure(&self, concurrency: u32, rate: Option<RatePolicy>, now: i64) {
        self.repository
            .configure(limits(concurrency, rate.as_ref()), UnixMillis(now))
            .await
            .unwrap();
    }
}

fn limits(concurrency: u32, rate: Option<&RatePolicy>) -> Vec<SchedulingLimit> {
    [
        SchedulerScope::Global,
        SchedulerScope::Tournament("tournament".to_owned()),
        SchedulerScope::Harness("codex".to_owned()),
        SchedulerScope::Provider("openai".to_owned()),
    ]
    .into_iter()
    .map(|scope| SchedulingLimit {
        schema_version: SCHEDULER_SCHEMA_VERSION,
        scope,
        max_concurrency: concurrency,
        rate: rate.cloned(),
    })
    .collect()
}

const fn rate(capacity: u32, refill_tokens: u32, refill_interval_ms: i64) -> RatePolicy {
    RatePolicy {
        capacity,
        refill_tokens,
        refill_interval_ms,
    }
}

fn request(reservation_id: &str, match_id: &str, now: i64, duration_ms: i64) -> ReservationRequest {
    ReservationRequest {
        schema_version: SCHEDULER_SCHEMA_VERSION,
        reservation_id: reservation_id.to_owned(),
        job_id: format!("job-{match_id}"),
        tournament_id: "tournament".to_owned(),
        match_id: match_id.to_owned(),
        run_id: format!("run-{match_id}"),
        harness_id: "codex".to_owned(),
        provider_id: "openai".to_owned(),
        immutable_inputs_sha256: "a".repeat(64),
        owner: "worker".to_owned(),
        now: UnixMillis(now),
        duration_ms,
    }
}

fn terminal(match_id: &str, marker: &str) -> TerminalMatchResult {
    TerminalMatchResult {
        schema_version: SCHEDULER_SCHEMA_VERSION,
        match_id: match_id.to_owned(),
        immutable_inputs_sha256: "a".repeat(64),
        result_sha256: "b".repeat(64),
        charge_key: format!("charge-{marker}"),
        telemetry_key: format!("telemetry-{marker}"),
        rating_key: format!("rating-{marker}"),
    }
}

fn acquired(result: ReservationResult) -> ExecutionReservation {
    let ReservationResult::Acquired(reservation) = result else {
        panic!("reservation must acquire")
    };
    *reservation
}
