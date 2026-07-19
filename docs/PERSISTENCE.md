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

Run them with:

```bash
cargo test -p word-arena-persistence --all-features
```
