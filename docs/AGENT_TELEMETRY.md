# Agent run telemetry

Word Arena archives only typed, visible agent activity. The private V1 archive,
public analytics projection, source labels, availability markers, and limits are
pinned in
[`contracts/agent-run-telemetry-v1.json`](../contracts/agent-run-telemetry-v1.json).
Hidden chain-of-thought is neither requested nor representable.

## Capture boundary

At terminal run completion, orchestration passes the driver's typed
`DriverTelemetry` through `RunTelemetryArchive::capture`. The capture is linked
to the exact manifest plus tournament, match, game, run, seat, and ordered turn
IDs. `SqliteAgentAttributionRepository::record_run_telemetry` checks that whole
correlation against immutable SQL rows and accepts the archive only after the
matching run result is terminal.

The private archive contains bounded visible inputs and outputs, normalized
MCP/tool calls, lifecycle events, injected-clock timings, restart count,
diagnostics/failures, and provider usage/cost when supplied. Each category has
a stable source label. Provider metrics use `exact`, `estimated`, or
`unavailable`; unavailable values have no numeric value and estimates are never
presented as exact. `SourcedU64::checked_sum` rejects cost/token overflow.

## Sanitization and limits

`TelemetrySanitizer` receives trusted in-memory secret material such as the
seat capability and any provider token known to orchestration. It never exposes
those values in `Debug`. Capture replaces configured values, common bearer/API
token forms, and values under sensitive JSON keys. Invalid UTF-8 becomes the
Unicode replacement character, unsafe control characters are replaced, and
text, JSON, turn, tool-call, diagnostic, and lifecycle counts are bounded.
Redaction/truncation counters and the policy version remain in the archive.

The sanitizer must run after the driver/workspace stream redactor and before
SQL persistence or structured logging. Raw driver telemetry and private archive
content must not be logged. Persistence reparses the strict schema and rejects
sequence, time, size, control-character, sensitive-key, availability, source,
or column drift on load.

## Public analytics and exports

`public_projection` is a separate Rust type. It includes exact correlation and
manifest identity, turn timings, restart count, provider usage/cost markers,
tool names, failure codes, source labels, and privacy metadata. These fields are
structurally absent:

- visible input and output, including rack text;
- diagnostic text;
- tool arguments and results.

This projection is the only run-telemetry type allowed in public analytics or
exports. Even normalized private content is never copied to a public payload.

## Retention

Each archive is either retained or carries an absolute expiry. Migration 7
stores this policy beside the schema and redaction versions.
`purge_expired_run_telemetry` transactionally removes both the detailed archive
and its budget snapshot at expiry. Game/replay records and manifest attribution
remain independent reproducibility inputs.

## Verification

```bash
cargo test -p word-arena-agent-runtime --all-features --test telemetry
cargo test -p word-arena-persistence --all-features
```

The suites cover a generated secret corpus, token shapes, sensitive JSON keys,
invalid UTF-8, control characters, truncation, availability, checked cost
arithmetic, ordering, identity drift, repository restart, retention, and public
export privacy.
