# Scoped Glicko-2 ratings

Word Arena ratings use Glicko-2 with immutable rating periods. Ratings are
separate for every language, exact ruleset identity, and rated-format policy.
Changing any one of those values creates a different pool; results never flow
between English and French or between single-game and paired formats.

The machine-readable V1 parameters and fixed-point representation are in
`contracts/ratings-v1.json`.

## Period inputs

A period records its pool and sequence, every contributing match identity and
score, and every entrant's previous rating plus opponent results. Validation
reconstructs the opponent multiset from the match rows. Hidden, missing, or
duplicate games are rejected before calculation.

Each game in a paired seat-swap series is an individual immutable input. Both
games enter the configured period once: the second game reverses seat identity,
but is not merged with or substituted for the first. A tie is exactly 500,000
score millionths. An entrant with no matches receives the standard inactivity
deviation update.

## Deterministic numbers

Calculation uses the standard Glicko-2 scale of `173.7178`, system constant
`tau = 0.5`, and volatility convergence tolerance `0.000001`. The published
reference example is locked by a test at approximately rating `1464.06`,
deviation `151.52`, and volatility `0.059996`.

Floating point exists only inside one calculation. Persisted and serialized
values use integer fixed point: milli-points for rating and deviation,
nano-units for volatility, and millionths for scores. Outputs are rounded once
at the period boundary. Safe public bounds are enforced, and derived deviation
is capped at the conventional 350-point ceiling.

## Persistence and audit

Migration 11 stores the canonical period payload, its versioned derived output,
and normalized match/input/update rows in one SQLx transaction. The current
rating table advances in the same transaction. Repeating an identical period
is idempotent; changing an existing period, reusing a match in a pool, skipping
a sequence, or presenting a stale previous rating is a conflict.

Loading cross-checks canonical JSON, derived calculations, headers, and every
normalized row. Rebuild processes periods in sequence, recomputes all updates,
checks the previous-rating chain, and compares the result with current ratings.
This makes current ratings a rebuildable projection rather than a separate
source of truth.

## Focused verification

```bash
cargo test -p word-arena-application --all-features --test rating
cargo test -p word-arena-persistence --all-features --test ratings
```
