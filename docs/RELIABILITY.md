# Mutation reliability

Word Arena treats SQLite as the authority for retries, deadlines, and recovery.
The deterministic engine remains clock-free and storage-free.

## Idempotency

Every create or game-action mutation requires an opaque idempotency key. The
application persists only its SHA-256 digest. A canonical command payload hash,
command kind, digest version, and versioned exact accepted or rejected outcome
are stored with it.

- Repeating the same key and payload returns the original outcome.
- Reusing a key with another payload returns `idempotency_conflict`.
- Accepted engine state, events, snapshots, deadlines, attempt counters, replay
  artifacts, and the outcome commit in one SQLite transaction.
- Transient repository failures are never cached.

Game creation uses a global key record because the generated game ID is not
known to a retrying client. Game actions scope keys to their game.

## Deadlines and invalid attempts

`OperationalPolicy` is injected at application bootstrap and carries a positive
version. It defines the turn duration, timeout response (`pass` or `resign`),
invalid-attempt limit, and invalid-attempt response (`reject_only`, `pass`, or
`resign`).

Each active version has one persisted deadline. The server scans due deadlines
in bounded batches after startup and every 250 ms. A player action and timeout
share the same optimistic version, so only one can commit. Timeout retry keys
are derived from game, turn, and policy version and therefore resolve once.

Invalid attempts are counted per game, turn, and seat. A rejected submitted
move never mutates the engine. If policy selects a forced pass or resignation,
that separate policy response is applied from the original snapshot and is
committed atomically with the rejected outcome. Counter updates use optimistic
compare-and-set behavior so concurrent attempts cannot lose increments.

## Recovery

Active games restart from authoritative private snapshots and event history.
They fail closed if private checkpoint bytes are corrupt. Finished games also
persist the engine's portable replay bundle; only finished games may do so
because the artifact contains the seed reveal. When a finished snapshot is
corrupt, the application verifies and replays that artifact against the exact
ruleset and lexicon pack.
