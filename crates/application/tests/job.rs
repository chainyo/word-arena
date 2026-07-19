use word_arena_application::{
    JOB_SCHEMA_VERSION, JobError, JobStatus, NewJob, UnixMillis, retry_backoff_ms,
};

#[test]
fn durable_job_input_requires_canonical_bounded_json() {
    let valid = job(br#"{"match_id":"match-1"}"#);
    valid.validate().unwrap();
    assert_eq!(valid.payload_sha256().unwrap().len(), 64);

    let mut whitespace = valid.clone();
    whitespace.payload_json = br#"{ "match_id": "match-1" }"#.to_vec();
    assert_eq!(whitespace.validate(), Err(JobError::InvalidJob));
    let mut unknown_kind = valid.clone();
    unknown_kind.kind = "Match Runner".to_owned();
    assert_eq!(unknown_kind.validate(), Err(JobError::InvalidJob));
    let mut unbounded = valid;
    unbounded.max_attempts = 101;
    assert_eq!(unbounded.validate(), Err(JobError::InvalidJob));
}

#[test]
fn retry_backoff_is_exponential_saturating_and_bounded() {
    assert_eq!(retry_backoff_ms(10, 1_000, 1), 10);
    assert_eq!(retry_backoff_ms(10, 1_000, 2), 20);
    assert_eq!(retry_backoff_ms(10, 1_000, 7), 640);
    assert_eq!(retry_backoff_ms(10, 1_000, 100), 1_000);
}

#[test]
fn terminal_statuses_are_explicit() {
    assert!(!JobStatus::Queued.is_terminal());
    assert!(!JobStatus::Leased.is_terminal());
    for status in [
        JobStatus::Succeeded,
        JobStatus::PermanentFailure,
        JobStatus::Exhausted,
        JobStatus::Cancelled,
    ] {
        assert!(status.is_terminal());
    }
}

fn job(payload_json: &[u8]) -> NewJob {
    NewJob {
        schema_version: JOB_SCHEMA_VERSION,
        job_id: "job-1".to_owned(),
        kind: "match.run".to_owned(),
        payload_schema_version: 1,
        payload_json: payload_json.to_vec(),
        priority: 0,
        available_at: UnixMillis(1),
        max_attempts: 3,
        retry_initial_ms: 100,
        retry_max_ms: 1_000,
        deduplication_key: "match-1".to_owned(),
    }
}
