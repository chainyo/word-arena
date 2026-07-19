# Deterministic tournament schedules

Tournament scheduling is an application concern built from immutable,
versioned inputs. It does not draw tiles, run agents, or decide game results.
The V1 contract is summarized in
[`contracts/tournament-schedule-v1.json`](../contracts/tournament-schedule-v1.json).

## Inputs and identity

A `TournamentSpec` contains a stable tournament ID, seeded entrants, one or
more ordered game profiles, a format, and an ordered commitment for every game
seed that can be scheduled. Each profile binds a language to exact ruleset and
Word Arena lexicon identities. Unknown fields, duplicate entrant IDs/seeds,
invalid hashes, zero-length formats, and ambiguous seed counts are rejected.

The format identity hashes the V1 schema version, format policy, and ordered
profiles. Entrants, tournament ID, and seed commitments are schedule-instance
inputs rather than format identity. Replays and analytics can therefore group
compatible formats without weakening an individual schedule's exact inputs.

## Formats and fairness

- Round robin uses a stable circle schedule and covers every pair once per
  cycle. Odd entrant counts rotate one bye through every entrant.
- Paired seat-swap schedules two games for each pairing with exact reversed
  seats and the same profile.
- Configurable series alternate seats for any positive game count; paired-swap
  series require an even game count.
- Swiss generates one round at a time from explicit standings, prior pairings,
  prior byes, seat counts, and next seed/sequence positions. Its stable order is
  match points, spread, wins, entrant seed, then entrant ID. Rematches either
  fail closed or are allowed only when required by the configured policy.

Static schedules assign profiles by cyclic distance between seeded entrants and
rotate that mapping across cycles. This balances each entrant's language and
ruleset exposure where the pairing graph permits it. Swiss schedules prefer the
seat orientation with the lowest recorded exposure and assign profiles
deterministically within each generated round, with stable input-order ties.

Every match has an immutable sequence, round/table/series position, exact seat
assignment, exact profile, and one SHA-256 game-seed commitment. Concurrent
waves are identified by round and series-game number, so an entrant is never
assigned twice in one wave.

## Persistence and lifecycle

`TournamentRepository` is the application port. The SQLx adapter writes the
spec, generated schedule, entrants, series, matches, seats, byes, and initial
lifecycle in one SQLite transaction. Ruleset and lexicon foreign keys must
already exist. A failed dependency or constraint rolls the entire insert back.

Loading reparses strict typed JSON, regenerates the expected schedule, and
compares every normalized row. Drift is reported as corrupt storage rather than
silently accepted. Lifecycle transitions use an expected sequence, legal state
edges, and monotonic timestamps in one transaction; stale writers conflict.
Swiss progress is stored beside the exact next generated round so restart does
not recompute from mutable match rows.

Migration 8 adds the schedule, normalized series/match/seat/bye, and lifecycle
tables while retaining the operations tables introduced earlier. Scheduled
matches are pending execution jobs; claiming and worker leases belong to
TOUR-002.

## Verification

Run the focused suites with:

```bash
cargo test -p word-arena-application --all-features --test tournament
cargo test -p word-arena-persistence --all-features --test tournaments
```

Golden and property tests cover byte determinism, pair coverage, rotating byes,
seat and profile balance, simultaneous-assignment uniqueness, Swiss tie breaks,
rematch behavior, strict seed consumption, restart loading, transaction
rollback, stale lifecycle transitions, and normalized-row tamper detection.
