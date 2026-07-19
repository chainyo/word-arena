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

## Credential-bound queries

Public, competitive seat, trusted human spectator, and administrator requests
and results are separate Rust types. Query bodies contain a game ID but no role
or seat selector. Every method requires a non-serializable, unforgeable
application credential bound to the same game:

- `PublicViewerCredential` maps only to the public projection;
- `CompetitiveSeatCredential` is fixed to exactly one seat projection and is
  the only credential accepted by game actions;
- `HumanSpectatorCredential` maps only to the trusted-human both-rack view;
- `AdministratorCredential` maps only to the authoritative checkpoint.

The sealed `Authorizes<Query>` matrix makes each credential/query pairing
explicit and prevents downstream crates from adding role mappings. Constructors
and fields are private, so transport code cannot construct or infer a credential
from request fields. APP-005 will authenticate opaque network capabilities and
map them to these application types.

## Operator separation

`ApplicationRuntime` is the trusted process-bootstrap boundary. It alone can
issue bearer capabilities after loading the bound game. Human-spectator and
administrator application credentials are produced only by authenticating a
correctly scoped capability; there is no unaudited direct issuance path. Agent
drivers and transport handlers receive `ApplicationService` plus a
`CompetitiveGameCredentials` value containing one seat and its public viewer;
that shape cannot contain or request an operator credential.

Game creation returns public and both seat credentials to trusted orchestration,
but never returns spectator or administrator credentials. The runtime and its
HMAC key must remain outside every agent process and workspace. See
[`CAPABILITIES.md`](CAPABILITIES.md) for issuance, authentication, rotation, and
audit rules.

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

The V1 Axum adapter authenticates headers into these credential types, derives
the acting seat server-side, and returns typed projections. Its WebSocket path
publishes only public version invalidations; clients always refresh from the
authoritative REST query. See [`API_V1.md`](API_V1.md).

## Verification

`crates/application/tests/application.rs` covers complete English/French games,
placement/exchange/pass/resignation routing, every allowed and denied
credential/query pairing, cross-game and cross-seat reuse, operator issuance,
stale versions, missing games/packs, rejected-action atomicity, exact timestamps,
and serialization isolation among credentials and result types.

Run it with:

```bash
cargo test -p word-arena-application --all-features
```
