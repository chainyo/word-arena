# Application command and query boundary

The `word-arena-application` crate is the only layer that coordinates engine
games for future REST, WebSocket, MCP, CLI, and worker adapters. It has no Axum,
MCP, SQLx, filesystem, random-device, or wall-clock dependency.

## Commands

`ApplicationService::prepare_create_game` obtains a validated game ID from the
injected source. `CreateGameCommand` records that ID, the immutable language,
and an idempotency key before creation. `GameActionCommand` records the target
game, expected version, explicit turn number/seat, idempotency key, and one typed
engine `Move`.

The service resolves the exact ruleset-bound lexicon, obtains a private seed,
and delegates every game transition to `word-arena-engine`. A successful action
replaces the repository checkpoint using its expected version and a single
application-clock timestamp. The SQLite adapter appends the resulting public
and private events and authoritative snapshot in one transaction. Idempotency
outcome persistence remains assigned to APP-007; the required command identity
is already part of every API.

## Authority-bound queries

Public, competitive seat, trusted human spectator, and administrator requests
and results are separate Rust types. Query bodies contain a game ID but no role
or seat selector. Nonpublic methods require a non-serializable binding created
with the game:

- `SeatAuthority` is fixed to exactly one game and seat;
- `HumanSpectatorAuthority` has no competitive-seat representation;
- `AdministratorAuthority` is a distinct authoritative checkpoint boundary.

These bindings are intentionally trusted, in-process precursors—not network
credentials. APP-004 and APP-005 will bind the same typed query methods to
unforgeable application credentials and opaque capabilities. Transport code
must not construct or infer an authority from request fields.

## Injected ports

- `GameRepository` inserts, loads, and optimistic-version-replaces complete
  `StoredGame` checkpoints through sendable futures.
- `LexiconResolver` returns an immutable validator only for an exact complete
  pack identity.
- `GameIdSource`, `SeedSource`, and `ApplicationClock` keep nondeterministic
  values outside the engine and make application tests repeatable.

The feature-gated `test_support` module supplies optimistic in-memory storage,
an exact fixture lexicon resolver, sequence IDs/seeds, and a fixed clock. It is
not compiled into default production builds.

## Verification

`crates/application/tests/application.rs` covers complete English/French games,
placement/exchange/pass/resignation routing, wrong-game and wrong-seat bindings,
stale versions, missing games/packs, rejected-action atomicity, exact timestamps,
and serialization isolation among public, seat, and spectator result types.

Run it with:

```bash
cargo test -p word-arena-application --all-features
```
