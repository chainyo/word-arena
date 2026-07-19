# Scheduling controls and worker recovery

The V1 scheduler composes with the durable job queue: jobs provide fair ordered
work and bounded retry, while execution reservations enforce capacity and
provider rate policy before a worker starts a match. The contract is summarized
in [`contracts/scheduler-controls-v1.json`](../contracts/scheduler-controls-v1.json).

## Limits and deterministic time

Every execution acquires four scopes atomically: global, tournament, harness,
and model provider. Each scope has a concurrency ceiling and an optional token
bucket. Buckets use integer tokens plus a persisted fractional remainder, so
refill and retry times are exact without floating point. All decisions use an
injected Unix-millisecond value; storage never reads wall-clock time.

An acquisition either reserves all four scopes and consumes all configured
tokens or reserves none. Expired reservations are recovered before capacity is
counted. A match can have only one live reservation.

## Control and recovery

Tournament controls are durable and sequenced. `paused` blocks new work,
`draining` blocks new work while existing reservations finish, and `cancelled`
marks every live reservation as cancellation-requested. Cancellation-requested
workers cannot renew or publish a terminal result, but may release their
capacity after cleanup. Cancelled control is terminal.

`reconstruct(now)` expires dead workers and returns ordered active reservations,
including the job, tournament, match, run, harness, provider, owner, immutable
input hash, expiry, and cancellation signal. This is enough to rebuild worker
supervision after process restart without changing scheduled game inputs.

## Exactly-once terminal publication

Every retry for a match must repeat its first persisted immutable-input hash.
Terminal publication validates a live reservation and atomically stores the
result hash plus unique charge, telemetry, and rating update keys while
releasing the reservation. Repeating the exact publication is idempotent;
changing any identity conflicts. Unique downstream keys prevent one worker race
from duplicating costs, telemetry, or future rating application.

Migration 10 stores policies, bucket state, tournament controls, normalized
reservation scopes, reservations, and terminal match results. SQLite write
transactions serialize the capacity check and reservation insert; no in-memory
counter is authoritative.

## Verification

```bash
cargo test -p word-arena-application --all-features --test scheduler
cargo test -p word-arena-persistence --all-features --test scheduler
```

Tests cover fractional refill, provider throttling, four-scope concurrency,
simultaneous acquisition, pause/resume/drain/cancel, cancellation-versus-result
races, worker expiry, restart reconstruction, immutable retry inputs, terminal
idempotency, and downstream key deduplication. Job fairness and bounded retry
remain covered by the TOUR-002 job suite.
