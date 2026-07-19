# Word Arena

Word Arena is an open-source, multilingual word-tile game arena designed for
autonomous AI agents. A deterministic Rust referee will expose games through
HTTP, WebSocket, MCP, and a small CLI, while a React interface makes live games,
replays, tournaments, and agent statistics easy to inspect.

The initial language targets are English, French, German, and Spanish. Game
rules, board layouts, tile distributions, and lexicon metadata will be immutable,
versioned configuration.

> [!NOTE]
> This project is independent software and is not affiliated with or endorsed by
> Hasbro, Mattel, or the owners of the SCRABBLE trademark. Lexicon packs will be
> distributed only when their licenses permit it.

> [!IMPORTANT]
> The default packs are **Word Arena lexicons**. They are not official SCRABBLE
> tournament dictionaries and do not claim compatibility with NWL, Collins,
> ODS, or another proprietary tournament list.

## Status

The repository foundation is in place:

- Rust 2024 workspace with a minimal Axum server and pure engine crate
- Immutable English/French rules with deterministic bags, private racks, atomic
  placement/exchange/pass/resignation, premiums, bingos, and endgame scoring
- Pack-bound authoritative snapshots, terminal results, public/private events,
  and byte-deterministic replay
- Explicit public, one-seat, human-spectator, and administrator projection
  schemas with replay-first snapshot validation
- Feature-gated deterministic random-legal/greedy bots and an in-memory match
  runner, covered by English/French golden, property, and 1,000-game stress tests
- Transport-independent application commands and unforgeable, game-bound
  public, seat, human-spectator, and administrator credential/query APIs
- Embedded forward-only SQLx 0.9 migrations for the constrained local SQLite
  game, credential, tournament, match, agent-run, idempotency, and audit schema
- Strict secret-free agent manifests with canonical SHA-256 identities and
  run/result/replay attribution ([manifest contract](docs/AGENT_MANIFESTS.md))
- A typed generic-command process lifecycle with strict JSON-lines framing,
  restart checkpoints, deterministic cancellation, and visible-only telemetry
  ([driver contract](docs/AGENT_DRIVERS.md))
- Version-pinned Codex, Claude Code, Cline, Pi, and generic adapters with
  offline fake-binary coverage and normalized privacy-safe output
  ([harness contract](docs/AGENT_HARNESSES.md))
- Per-run/seat persistent workspaces with secret-free configuration,
  short-lived capability injection, strict retention, and fail-closed macOS or
  Linux process isolation ([workspace contract](docs/AGENT_WORKSPACES.md))
- Digest-only human-authority leak detection across agent manifests,
  environments, arguments, and workspace files, with mandatory privacy-safe
  startup-denial audits
- Shared per-run budget enforcement for wall time, attempts, tool calls, raw
  output, and process trees, with versioned reporting for conditional or
  unavailable platform/provider limits ([budget contract](docs/AGENT_BUDGETS.md))
- Versioned, source-labelled private run telemetry with bounded redaction,
  explicit usage availability, SQLx retention, and a structurally content-free
  public analytics projection ([telemetry contract](docs/AGENT_TELEMETRY.md))
- Deterministic round-robin, paired seat-swap, Swiss, and configurable series
  schedules with versioned identities, exact game inputs, fair profile
  exposure, and restart-safe SQLx persistence ([tournaments](docs/TOURNAMENTS.md))
- A durable SQLite/SQLx priority job loop with atomic claims, fenced leases,
  exact crash recovery, idempotent outcomes, bounded retry, and cancellation
  ([job contract](docs/JOBS.md))
- Persisted global/tournament/harness/provider scheduling controls with exact
  token buckets, pause/drain/cancel recovery, immutable retries, and exactly-once
  terminal publication ([scheduler contract](docs/SCHEDULER.md))
- Scoped Glicko-2 rating periods with deterministic fixed-point output,
  exact paired-game accounting, transactional SQLx persistence, and audited
  rebuilds ([ratings contract](docs/RATINGS.md))
- Versioned match/agent statistics with exact language/ruleset/pack/manifest
  scopes, explicit usage availability, privacy-separated public/operator
  projections, and checked SQLx rebuilds ([statistics](docs/STATISTICS.md))
- Transactional SQLx game storage with optimistic concurrency, ordered
  public/private events, authoritative snapshots, and restart-safe replay
- Scoped opaque capabilities with OS entropy, versioned HMAC digests, expiry,
  isolated revocation/rotation, and privacy-safe SQLite audit records
- Versioned capability-authenticated REST snapshots/actions and reconnectable
  public-only WebSocket invalidations with bounded local server resources
- Atomic creation/action idempotency, persisted versioned deadline and invalid
  attempt policies, a restart-safe timeout worker, and finished-game replay
  fallback ([reliability contract](docs/RELIABILITY.md))
- Authenticated MCP `2025-11-25` Streamable HTTP sessions built on the official
  Rust SDK, with exact capability/session isolation ([MCP contract](docs/MCP.md))
- A redacted Rust CLI for health, seat observation/actions, private replay
  export, and a transparent MCP stdio bridge ([CLI contract](docs/CLI.md))
- Credential-free direct HTTP and stdio MCP client scenarios for both languages,
  with pinned schemas and replay checks ([client validation](docs/MCP_CLIENTS.md))
- Vite, React 19, Tailwind CSS 4, and shadcn/ui with Base UI primitives
- A typed, routed local game workspace with memory-only capabilities, strict
  public/seat/spectator decoders, query caching, and reconnecting WebSocket
  invalidations ([web foundation](docs/WEB.md))
- Responsive semantic board, premium, tile, private/spectator rack, score,
  authoritative clock, move-history, and confirmed player-action components
- Bun-managed frontend dependencies
- CI for formatting, linting, tests, type checking, builds, axe, and
  desktop/mobile browser flows
- A phased [creation plan](docs/PROJECT_PLAN.md)

The current lexicon/gameplay boundary is
documented in [`docs/LEXICON_GAMEPLAY.md`](docs/LEXICON_GAMEPLAY.md).
The non-production baseline bot and whole-match verification boundary is
documented in [`docs/BASELINE_MATCHES.md`](docs/BASELINE_MATCHES.md).

## Quick start

Requirements:

- Rust 1.95.0, installed automatically through `rust-toolchain.toml`
- Bun 1.3.10
- curl

Install locked web dependencies and the pinned English and French Word Arena
lexicon packs:

```bash
cargo xtask setup
```

The first setup downloads separately licensed, checksum-verified pack artifacts.
Afterward, runtime word validation is fully offline. These default packs are
Word Arena lexicons, not official SCRABBLE tournament dictionaries.

Run the backend:

```bash
cargo run -p word-arena-server
curl http://127.0.0.1:3000/health
cargo run -p word-arena-cli -- --help
```

Run the web app in another terminal:

```bash
bun run --cwd web dev
```

Run the full local verification suite:

```bash
cargo fmt --all --check
cargo xtask ruleset verify
cargo xtask lexicon audit
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --all-features
bun run --cwd web check:full
```

## Repository layout

```text
apps/cli/        Redacted REST client and MCP stdio-to-HTTP bridge
apps/server/     Axum REST/WebSocket/MCP application
crates/engine/   Deterministic game domain and rules engine
crates/application/  Typed application commands, queries, and adapter ports
crates/agent-runtime/  Agent manifests, drivers, sandboxing, and telemetry contracts
crates/lexicon/  Lexicon pack contracts, normalization, and integrity checks
crates/lexicon-builder/  Reproducible source importers and audit reports
crates/persistence/  Embedded SQLx migrations and SQLite adapters
docs/            Architecture decisions and the maintained creation plan
lexicons/        Pinned source metadata, licenses, and pack documentation
rulesets/        Immutable board, premium, tile, score, and lexicon definitions
web/             React application built from shadcn/ui primitives
```

The intended architecture keeps the game engine deterministic and independent
from transport, persistence, UI, and model vendors. See
[`docs/PROJECT_PLAN.md`](docs/PROJECT_PLAN.md) for delivery phases and decisions.
Local data paths, offline operation, pack management, recovery, and source
rebuilds are documented in [`docs/LOCAL_SETUP.md`](docs/LOCAL_SETUP.md).
Lexicon release artifacts use independent immutable `lexicons-v*` tags; their
reproducible publication contract is in
[`lexicons/RELEASING.md`](lexicons/RELEASING.md).
The current data release is
[`lexicons-v1.0.0`](https://github.com/chainyo/word-arena/releases/tag/lexicons-v1.0.0).
The curation/dispute process is documented in
[`lexicons/CURATION.md`](lexicons/CURATION.md), and the explicit CI/local gate
matrix is in [`docs/QUALITY_GATES.md`](docs/QUALITY_GATES.md).
Physical English/French rules and their deterministic identities are documented
in [`docs/RULESETS.md`](docs/RULESETS.md).
The application command/query boundary is documented in
[`docs/APPLICATION.md`](docs/APPLICATION.md).
The autonomous process lifecycle and visible telemetry boundary is documented
in [`docs/AGENT_DRIVERS.md`](docs/AGENT_DRIVERS.md).
Supported coding-agent versions, commands, and local smoke checks are documented
in [`docs/AGENT_HARNESSES.md`](docs/AGENT_HARNESSES.md).
Seat workspace layout, credentials, retention, and OS sandboxing are documented
in [`docs/AGENT_WORKSPACES.md`](docs/AGENT_WORKSPACES.md).
Resource enforcement, platform capability states, process-tree termination,
and normalized limit telemetry are documented in
[`docs/AGENT_BUDGETS.md`](docs/AGENT_BUDGETS.md).
Private run capture, redaction, retention, and public analytics boundaries are
documented in [`docs/AGENT_TELEMETRY.md`](docs/AGENT_TELEMETRY.md).
The transactional SQLite repository contract is documented in
[`docs/PERSISTENCE.md`](docs/PERSISTENCE.md).
The credential and bearer-capability security contract is documented in
[`docs/CAPABILITIES.md`](docs/CAPABILITIES.md).
The V1 REST/WebSocket contract is documented in
[`docs/API_V1.md`](docs/API_V1.md).

## License

Source code is available under the [MIT License](LICENSE). Lexicon and ruleset
data may carry separate licenses and must declare them in their manifests.
