# ADR-0001: SQLite persistence through SQLx

- Status: accepted
- Date: 2026-07-19
- Scope: application persistence and local worker coordination

## Context

Word Arena is local-first and needs transactional snapshots/events, optimistic
concurrency, restart recovery, tournament scheduling, and statistics without
operating a separate database service. The engine and application command types
must remain independent from the selected store.

## Decision

Use SQLite as the first datastore and SQLx 0.9.0 as the async Rust adapter. SQLx
0.9.0 declares Rust 1.94 as its minimum and is dual MIT/Apache-2.0 licensed,
which is compatible with this workspace's Rust 1.95 toolchain and MIT source.
The workspace disables SQLx defaults and enables only `runtime-tokio`,
`sqlite-bundled`, and `migrate`.

Migrations are forward-only SQL files embedded with `include_str!` into SQLx
`Migration` values. This avoids enabling the broader SQLx macro feature while
retaining migration checksums/history and a self-contained binary. SQL files are
forced to LF in `.gitattributes` because migration identity hashes their bytes.
Startup opens a bounded pool, enables foreign keys, applies and validates all
migrations, then makes the pool available to adapters.

The initial schema uses:

- immutable ruleset and exact lexicon-pack identities referenced by games and
  matches;
- separate public/private event and versioned snapshot records;
- constrained game seats and future agent-run/tournament relationships;
- 64-bit UTC Unix milliseconds, avoiding host-local time interpretation;
- 32-byte opaque capability/idempotency digests only—never raw bearer tokens;
- strict tables, foreign keys, checks, unique constraints, and query indexes;
- explicit application schema metadata in addition to SQLx migration history.

Official references:

- [SQLx SQLite module](https://docs.rs/sqlx/0.9.0/sqlx/sqlite/)
- [SQLx migration module](https://docs.rs/sqlx/0.9.0/sqlx/migrate/)
- [`migrate!` embedding and hash guidance](https://docs.rs/sqlx/0.9.0/sqlx/macro.migrate.html)
- [SQLx upstream repository](https://github.com/launchbadge/sqlx)

## Consequences

- One local file and normal filesystem backups are sufficient for V1.
- WAL supports concurrent readers while the application serializes conflicting
  writers through expected versions and transactions.
- The bundled SQLite build provides a consistent supported engine across local
  platforms at the cost of additional compile time and binary size.
- SQLx remains isolated in `word-arena-persistence`; application use cases see
  repository errors and domain records rather than database rows or queries.
- Redis, Kafka, NATS, PostgreSQL, and a standalone queue remain deferred until
  measured operational requirements justify them.

## Verification

Temporary-file integration tests apply the real embedded migrations twice,
assert ordered versions and every required table, exercise foreign-key/check
and secret-digest constraints, and prove a deliberately broken migration rolls
back its partial DDL. Full workspace compilation embeds and validates migration
metadata in CI.
