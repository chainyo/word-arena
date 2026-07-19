# Transactional game persistence

`word-arena-persistence` implements the application `GameRepository` over
SQLite with SQLx. The repository stores immutable ruleset and lexicon-pack
identities beside each game so a checkpoint cannot silently resume against
different gameplay data.

## Creation and transitions

Game creation registers the exact ruleset and lexicon metadata, then inserts
the game, seats, creation event, and version-zero snapshot in one transaction.
Duplicate game IDs and incompatible metadata fail without partial rows.

Every accepted action is written in one transaction guarded by
`games.version = expected_version`. The transaction:

1. advances the game version and update timestamp;
2. appends exactly one ordered public event and any corresponding private event;
3. preserves the previous checkpoint as historical data; and
4. inserts the new authoritative snapshot.

Two writers using the same expected version therefore cannot both commit. A
failed event or snapshot write rolls back the version update as well.

## Loading and diagnostics

Loading validates the current snapshot schema, replay/projection schema
versions, immutable ruleset and lexicon identities, complete event sequences,
and private-event ownership. Failures use stable categories: not found,
conflict, incompatible schema, incompatible pack, corrupt data, or transient
unavailability. Error values never include serialized private payloads.

The persisted snapshot remains authoritative for fast restart while the full
event history remains available for deterministic replay and audit. A replay
references the immutable lexicon pack identity; it does not embed a word list.

## Verification

Repository integration tests cover duplicate/missing games, concurrent writers,
transaction rollback, ordered public/private history, restart and resume,
byte-equivalent replay, incompatible schemas, pack mismatch, corruption, and
unavailable storage.

The same crate implements capability storage as digest-only records. Issuance,
revocation, and rotation update their privacy-safe audit rows in the same SQL
transaction. Migration 3 adds the optional foreign-keyed agent-run binding
without rewriting the already-published operations migration.

Migration 5 content-addresses canonical agent manifests and repeats the exact
manifest digest on terminal run results and per-seat replay attribution. Foreign
keys reject cross-run, cross-game, cross-seat, or changed-manifest attachment;
provider secrets are not representable in the validated manifest bytes stored
by the adapter.

Migration 6 stores one final normalized budget-telemetry snapshot only after
its run is terminal. The row repeats the exact manifest identity and both
budget schema versions; strict loading reparses the typed JSON and rejects
schema or ordered-limit-event drift.

Migration 7 stores one final sanitized run-telemetry archive after terminal
result creation. Repeated columns and the insert query bind tournament, match,
game, run, seat, and manifest identity. The typed archive carries explicit
retention; expiry transactionally removes detailed and budget telemetry. Public
reads return the content-free analytics projection, never the private JSON.

Migration 8 stores the immutable V1 tournament spec and schedule plus normalized
series, matches, seat assignments, byes, Swiss progress, and ordered lifecycle
events. Insert is one transaction with ruleset/lexicon foreign keys. Load
regenerates the deterministic schedule and rejects JSON, header, or normalized
row drift; lifecycle changes use sequence-based optimistic concurrency.

Migration 9 adds durable jobs and attempt history. Atomic claim-and-return
orders eligible work by priority and age, increments a lease fence, and records
each attempt. Injected-time renewal/completion rejects expired or stale owners;
expired attempts become reclaimable exactly at expiry. Deduplication and
idempotent completion are durable across process and database restart.

Migration 10 adds scheduler limits, integer token buckets, tournament controls,
execution reservations and normalized scopes, plus exactly-once terminal match
results. Capacity acquisition and token consumption are one transaction.
Recovery expires dead reservations from injected time; result, charge,
telemetry, and rating identities commit together.

Migration 11 adds immutable, scoped Glicko-2 periods plus normalized match,
entrant-input, derived-update, and current-rating rows. Period insertion and
current projection updates commit together. Loads and rebuilds recompute every
fixed-point result and reject canonical payload, normalized row, sequence, or
previous-rating drift.

Migration 12 adds immutable completed-game statistics sources and two
seat-scoped observations per source. Source and observations commit together;
load re-derives gameplay and telemetry totals and rejects JSON or normalized
scope drift. Public and operator aggregates rebuild from the same ordered,
checked source history.

Run them with:

```bash
cargo test -p word-arena-persistence --all-features
```
