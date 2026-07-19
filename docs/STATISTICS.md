# Match and agent statistics

Word Arena computes V1 statistics from immutable referee events, authoritative
invalid-attempt counters, and normalized content-free agent telemetry. It does
not inspect model transcripts or private racks. The machine-readable public
contract is `contracts/statistics-v1.json`.

## Source and metrics

One completed game creates two seat observations. The event stream determines
win/loss/tie, scores and spread, placement count and score, bingos, passes,
exchanges, premium-square use, and normalized formed-word usage. Invalid actions
come from the persisted per-turn counters. Turn durations, tool-call totals,
input/output tokens, and micro-USD cost come from privacy-safe run telemetry.

Average move score is based on scoring placement moves, excluding passes and
exchanges. Win rate is wins divided by games; ties are reported separately.
Rates use integer millionths and average move scores use integer milli-points,
so output order and rounding are stable across rebuilds.

## Scope and missing data

Filters can independently select language, exact ruleset identity, exact pack
identity, agent manifest, tournament, entrant, seat, and an inclusive-from,
exclusive-before completion window. English and French, or two pack/ruleset
versions, are never merged unless an operator deliberately leaves those
dimensions unfiltered.

Provider-derived values are explicitly `exact`, `estimated`, or `unavailable`.
An aggregate is unavailable when any selected observation is unavailable; it is
estimated when all observations have values and at least one is estimated.
This avoids presenting a partial token or cost total as complete.

## Privacy boundary

`PublicStatistics` contains only aggregate counters and vocabulary size. Word
keys, source IDs, racks, transcripts, diagnostics, tool arguments, and tool
results are structurally absent. `OperatorStatistics` is a separate authorized
type that adds normalized word frequencies and immutable source IDs for
drill-down; it still has no rack or transcript fields.

## Persistence and rebuild

Migration 12 stores each validated completed-game source, its two derived
observations, and normalized scope columns in one transaction. An identical
retry is idempotent and changed reuse of a source ID conflicts. Loading
re-derives observations and cross-checks every normalized column.

Full rebuild reads immutable sources in source-ID order and feeds the same
checked accumulator used for incremental updates. All counters use checked
arithmetic; overflow fails rather than wrapping.

```bash
cargo test -p word-arena-application --all-features --test statistics
cargo test -p word-arena-persistence --all-features --test statistics
```
