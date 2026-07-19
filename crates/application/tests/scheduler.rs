use word_arena_application::{
    RatePolicy, SchedulerError, TokenBucketState, UnixMillis, refill_bucket, token_retry_at,
};

#[test]
fn token_bucket_preserves_fractional_refill_exactly() {
    let policy = RatePolicy {
        capacity: 3,
        refill_tokens: 2,
        refill_interval_ms: 10,
    };
    let start = TokenBucketState {
        tokens: 0,
        remainder: 0,
        updated_at: UnixMillis(100),
    };
    let first = refill_bucket(start, &policy, UnixMillis(104)).unwrap();
    assert_eq!(first.tokens, 0);
    assert_eq!(first.remainder, 8);
    assert_eq!(
        token_retry_at(first, &policy, UnixMillis(104)),
        Some(UnixMillis(105))
    );
    let second = refill_bucket(first, &policy, UnixMillis(105)).unwrap();
    assert_eq!(second.tokens, 1);
    assert_eq!(second.remainder, 0);
    let full = refill_bucket(second, &policy, UnixMillis(1_000)).unwrap();
    assert_eq!(full.tokens, 3);
    assert_eq!(full.remainder, 0);
}

#[test]
fn token_bucket_rejects_clock_reversal() {
    let policy = RatePolicy {
        capacity: 1,
        refill_tokens: 1,
        refill_interval_ms: 10,
    };
    assert_eq!(
        refill_bucket(
            TokenBucketState {
                tokens: 0,
                remainder: 0,
                updated_at: UnixMillis(10),
            },
            &policy,
            UnixMillis(9),
        ),
        Err(SchedulerError::InvalidInput)
    );
}
