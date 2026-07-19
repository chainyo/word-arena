# Durable SQLite job loop

Word Arena uses a small SQLx/SQLite job repository before introducing any
network queue. The V1 boundary is summarized in
[`contracts/job-queue-v1.json`](../contracts/job-queue-v1.json).

## Durable input and deduplication

`NewJob` binds a stable kind, canonical JSON payload and schema version,
priority, availability time, retry bounds, and deduplication key. Payloads are
limited to 1 MiB and stored with an exact SHA-256. `(kind, deduplication_key)` is
unique. Re-enqueueing the exact same immutable input returns the existing job;
changing its payload or policy is a conflict.

All time is supplied through application inputs or `ApplicationClock`. The
repository never reads wall-clock time, which makes availability, expiry, and
backoff tests exact.

## Claim and lease rules

One atomic SQLite `UPDATE ... RETURNING` selects by priority descending, then
availability, creation time, and job ID ascending. Workers may restrict claims
to reviewed kinds. A claim increments both attempt and lease generation and
records a separate immutable attempt row.

The fence is `(job_id, attempt, lease_generation, worker_id)`. Renewal and
completion require that exact live fence. A lease remains live while its expiry
is strictly greater than injected `now`; it is reclaimable exactly at expiry.
Before a new claim, expired attempts are marked abandoned. A stale worker can
neither renew nor complete after another worker has reclaimed the job.

## Outcomes, retry, and cancellation

Handlers return one of four typed outcomes: succeeded, retryable with a stable
error code, permanent failure with a stable error code, or cancelled. Retryable
work returns to `queued` at:

```text
now + min(initial_backoff * 2^(attempt - 1), maximum_backoff)
```

The repository derives `exhausted` when the attempt limit is reached. Every
attempt stores the requested handler outcome and resulting durable state, so an
identical completion retry returns `AlreadyApplied` while a different retry
conflicts. This gives handlers a stable idempotency identity without treating a
stale lease as success.

Queued cancellation is immediately terminal. Leased cancellation records a
durable request; renewal reports it and the worker completes with `cancelled`.
Cross-process propagation and tournament pause/drain behavior are TOUR-003.

`JobWorker::run_once` is the minimal injected-clock loop: claim one supported
kind, run its handler, and durably classify the result. Operators may repeat it
until idle or place it inside their own shutdown-aware task loop.

## Persistence and verification

Migration 9 adds `jobs` plus ordered `job_attempts`. Strict checks constrain
state/lease shape, retry limits, terminal timestamps, payload size, hashes, and
outcome fields. SQLite WAL and busy timeouts remain the local concurrency
boundary.

Run the focused suites with:

```bash
cargo test -p word-arena-application --all-features --test job
cargo test -p word-arena-persistence --all-features --test jobs
```

They cover canonical input, deduplication, fair ordering, simultaneous claims,
renewal, exact expiry, crash/restart reclamation, stale fencing, idempotent
completion, bounded retries, exhaustion, permanent failure, cancellation, and
the injected-clock worker.
