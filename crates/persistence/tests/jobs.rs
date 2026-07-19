use std::{collections::BTreeSet, sync::Arc};

use tempfile::TempDir;
use word_arena_application::{
    ApplicationClock, BoxFuture, CancellationResult, ClaimJobs, CompletionResult, EnqueueResult,
    JOB_SCHEMA_VERSION, JobHandler, JobHandlerOutcome, JobLease, JobRepository, JobRepositoryError,
    JobStatus, JobWorker, NewJob, RenewalResult, UnixMillis, WorkerStep,
};
use word_arena_persistence::{SqliteJobRepository, connect_and_migrate};

#[tokio::test]
async fn enqueue_deduplicates_exactly_and_survives_restart() {
    let database = Database::open("enqueue").await;
    let first = database
        .repository
        .enqueue(job("job-1", "same", 0, 3), UnixMillis(1))
        .await
        .unwrap();
    assert!(matches!(first, EnqueueResult::Inserted(_)));
    let duplicate = database
        .repository
        .enqueue(job("different-generated-id", "same", 0, 3), UnixMillis(2))
        .await
        .unwrap();
    let EnqueueResult::Existing(existing) = duplicate else {
        panic!("retry must return the first durable job");
    };
    assert_eq!(existing.job_id, "job-1");

    let mut conflict = job("job-2", "same", 0, 3);
    conflict.payload_json = br#"{"match":"changed"}"#.to_vec();
    assert_eq!(
        database.repository.enqueue(conflict, UnixMillis(2)).await,
        Err(JobRepositoryError::Conflict)
    );
    let restarted = SqliteJobRepository::new(database.pool.clone());
    assert_eq!(restarted.load("job-1").await.unwrap(), existing);
}

#[tokio::test]
async fn concurrent_claimers_never_share_a_live_lease_and_order_is_fair() {
    let database = Database::open("claims").await;
    for index in 0..12 {
        let mut queued = job(&format!("job-{index:02}"), &format!("key-{index}"), 0, 2);
        queued.priority = if index < 2 { 10 } else { 0 };
        database
            .repository
            .enqueue(queued, UnixMillis(index))
            .await
            .unwrap();
    }
    let first = database
        .repository
        .claim(claim("first", 20, 100))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.job.job_id, "job-00");

    let repository = Arc::new(database.repository.clone());
    let mut tasks = Vec::new();
    for index in 0..16 {
        let repository = Arc::clone(&repository);
        tasks.push(tokio::spawn(async move {
            repository
                .claim(claim(&format!("worker-{index}"), 20, 100))
                .await
                .unwrap()
                .map(|lease| lease.job.job_id)
        }));
    }
    let mut claimed = BTreeSet::from([first.job.job_id]);
    for task in tasks {
        if let Some(job_id) = task.await.unwrap() {
            assert!(claimed.insert(job_id), "one live job was leased twice");
        }
    }
    assert_eq!(claimed.len(), 12);
}

#[tokio::test]
async fn expiry_renewal_fencing_and_idempotent_completion_are_exact() {
    let database = Database::open("leases").await;
    database
        .repository
        .enqueue(job("job", "lease", 0, 3), UnixMillis(0))
        .await
        .unwrap();
    let first = database
        .repository
        .claim(claim("one", 10, 10))
        .await
        .unwrap()
        .unwrap();
    assert!(
        database
            .repository
            .claim(claim("two", 19, 10))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        database
            .repository
            .renew(first.clone(), UnixMillis(19), 20)
            .await
            .unwrap(),
        RenewalResult::Renewed
    );
    assert!(
        database
            .repository
            .claim(claim("two", 38, 10))
            .await
            .unwrap()
            .is_none()
    );

    let restarted = SqliteJobRepository::new(database.pool.clone());
    let second = restarted
        .claim(claim("two", 39, 10))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.job.attempt, 2);
    assert_eq!(
        restarted
            .complete(first, JobHandlerOutcome::Succeeded, UnixMillis(40))
            .await,
        Err(JobRepositoryError::Conflict)
    );
    let applied = restarted
        .complete(second.clone(), JobHandlerOutcome::Succeeded, UnixMillis(40))
        .await
        .unwrap();
    assert!(matches!(applied, CompletionResult::Applied(_)));
    assert!(matches!(
        restarted
            .complete(second.clone(), JobHandlerOutcome::Succeeded, UnixMillis(41))
            .await
            .unwrap(),
        CompletionResult::AlreadyApplied(_)
    ));
    assert_eq!(
        restarted
            .complete(
                second,
                JobHandlerOutcome::Permanent {
                    error_code: "different".to_owned(),
                },
                UnixMillis(41),
            )
            .await,
        Err(JobRepositoryError::Conflict)
    );
}

#[tokio::test]
async fn retry_backoff_and_exhaustion_are_deterministic() {
    let database = Database::open("outcomes").await;
    database
        .repository
        .enqueue(job("retry", "retry", 0, 2), UnixMillis(0))
        .await
        .unwrap();
    let first = database
        .repository
        .claim(claim("worker", 0, 100))
        .await
        .unwrap()
        .unwrap();
    let CompletionResult::Applied(queued) = database
        .repository
        .complete(
            first,
            JobHandlerOutcome::Retryable {
                error_code: "busy".to_owned(),
            },
            UnixMillis(1),
        )
        .await
        .unwrap()
    else {
        panic!("first retry must apply")
    };
    assert_eq!(queued.status, JobStatus::Queued);
    assert_eq!(queued.available_at, UnixMillis(11));
    assert!(
        database
            .repository
            .claim(claim("worker", 10, 100))
            .await
            .unwrap()
            .is_none()
    );
    let second = database
        .repository
        .claim(claim("worker", 11, 100))
        .await
        .unwrap()
        .unwrap();
    let CompletionResult::Applied(exhausted) = database
        .repository
        .complete(
            second,
            JobHandlerOutcome::Retryable {
                error_code: "busy".to_owned(),
            },
            UnixMillis(12),
        )
        .await
        .unwrap()
    else {
        panic!("last retry must apply")
    };
    assert_eq!(exhausted.status, JobStatus::Exhausted);
}

#[tokio::test]
async fn permanent_and_cancellation_outcomes_are_distinct() {
    let database = Database::open("terminal-outcomes").await;
    database
        .repository
        .enqueue(job("permanent", "permanent", 0, 3), UnixMillis(20))
        .await
        .unwrap();
    let permanent = database
        .repository
        .claim(claim("worker", 20, 100))
        .await
        .unwrap()
        .unwrap();
    let CompletionResult::Applied(permanent) = database
        .repository
        .complete(
            permanent,
            JobHandlerOutcome::Permanent {
                error_code: "invalid".to_owned(),
            },
            UnixMillis(21),
        )
        .await
        .unwrap()
    else {
        panic!("permanent result must apply")
    };
    assert_eq!(permanent.status, JobStatus::PermanentFailure);

    database
        .repository
        .enqueue(job("queued-cancel", "queued-cancel", 20, 3), UnixMillis(20))
        .await
        .unwrap();
    assert_eq!(
        database
            .repository
            .cancel("queued-cancel", UnixMillis(21))
            .await
            .unwrap(),
        CancellationResult::Cancelled
    );
    database
        .repository
        .enqueue(job("leased-cancel", "leased-cancel", 20, 3), UnixMillis(20))
        .await
        .unwrap();
    let leased = database
        .repository
        .claim(claim("worker", 20, 100))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        database
            .repository
            .cancel("leased-cancel", UnixMillis(21))
            .await
            .unwrap(),
        CancellationResult::Requested
    );
    assert_eq!(
        database
            .repository
            .renew(leased.clone(), UnixMillis(22), 100)
            .await
            .unwrap(),
        RenewalResult::CancellationRequested
    );
    let CompletionResult::Applied(cancelled) = database
        .repository
        .complete(leased, JobHandlerOutcome::Cancelled, UnixMillis(23))
        .await
        .unwrap()
    else {
        panic!("cancel must apply")
    };
    assert_eq!(cancelled.status, JobStatus::Cancelled);
}

#[tokio::test]
async fn worker_uses_injected_clock_and_handler_contract() {
    let database = Database::open("worker").await;
    database
        .repository
        .enqueue(job("job", "worker", 50, 3), UnixMillis(1))
        .await
        .unwrap();
    let clock = Arc::new(FixedClock(UnixMillis(50)));
    let worker = JobWorker::new(
        Arc::new(database.repository.clone()),
        clock,
        Arc::new(SucceedingHandler("match".to_owned())),
        "worker".to_owned(),
        100,
    )
    .unwrap();
    let WorkerStep::Completed { result, .. } = worker.run_once().await.unwrap() else {
        panic!("available job must run");
    };
    let CompletionResult::Applied(job) = *result else {
        panic!("first completion must apply")
    };
    assert_eq!(job.status, JobStatus::Succeeded);
    assert_eq!(worker.run_once().await.unwrap(), WorkerStep::Idle);
}

#[derive(Debug)]
struct FixedClock(UnixMillis);

impl ApplicationClock for FixedClock {
    fn now(&self) -> UnixMillis {
        self.0
    }
}

#[derive(Debug)]
struct SucceedingHandler(String);

impl JobHandler for SucceedingHandler {
    fn kind(&self) -> &str {
        &self.0
    }

    fn handle<'a>(&'a self, _lease: &'a JobLease) -> BoxFuture<'a, JobHandlerOutcome> {
        Box::pin(async { JobHandlerOutcome::Succeeded })
    }
}

struct Database {
    _directory: TempDir,
    pool: sqlx::SqlitePool,
    repository: SqliteJobRepository,
}

impl Database {
    async fn open(label: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("{label}.sqlite3"));
        let pool = connect_and_migrate(&format!("sqlite://{}", path.display()))
            .await
            .unwrap();
        let repository = SqliteJobRepository::new(pool.clone());
        Self {
            _directory: directory,
            pool,
            repository,
        }
    }
}

fn job(job_id: &str, key: &str, available_at: i64, max_attempts: u32) -> NewJob {
    NewJob {
        schema_version: JOB_SCHEMA_VERSION,
        job_id: job_id.to_owned(),
        kind: "match".to_owned(),
        payload_schema_version: 1,
        payload_json: br#"{"match":"one"}"#.to_vec(),
        priority: 0,
        available_at: UnixMillis(available_at),
        max_attempts,
        retry_initial_ms: 10,
        retry_max_ms: 40,
        deduplication_key: key.to_owned(),
    }
}

fn claim(worker_id: &str, now: i64, lease_duration_ms: i64) -> ClaimJobs {
    ClaimJobs {
        worker_id: worker_id.to_owned(),
        kinds: BTreeSet::from(["match".to_owned()]),
        now: UnixMillis(now),
        lease_duration_ms,
    }
}
