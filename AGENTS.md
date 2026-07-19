# AGENTS.md

Word Arena is a multilingual word-tile game and tournament platform for humans
and autonomous agents. The backend is Rust; the web application is React with
shadcn/ui. Treat this file as durable repository guidance and update it when a
repeated correction should apply to future work.

## Start here

- Read `README.md` for setup and current repository shape.
- Read `docs/PROJECT_PLAN.md` before architectural or milestone changes.
- Check `git status` before editing and preserve changes you did not create.
- Keep one task per feature branch or worktree. Prefer pull requests after the
  initial bootstrap.
- Use Conventional Commits for commit messages and pull request titles.

## Repository map

- `crates/engine/`: deterministic domain model, validation, scoring, and events.
  This crate must not depend on HTTP, MCP, databases, model providers, or wall
  clock time.
- `crates/application/`: transport-agnostic commands, authority-bound queries,
  and injected repository/lexicon/ID/seed/clock ports. Database, HTTP, MCP, and
  raw credential parsing remain adapters outside its core use cases.
- `crates/lexicon/`: versioned pack contracts, normalization, integrity checks,
  and runtime exact-membership adapters. Keep pack parsing independent from the
  game engine and transport layers.
- `crates/lexicon-builder/`: deterministic, auditable source importers and pack
  build tooling. Generated word data and build output must stay outside Git.
- `crates/persistence/`: SQLite schema, SQLx migrations, and application port
  adapters. Keep migrations forward-only, embedded, constraint-heavy, and
  covered by temporary-database integration tests.
- `apps/server/`: process entry point and application adapters. HTTP, WebSocket,
  MCP, authentication, persistence, and observability belong here or in focused
  crates extracted from it later.
- `apps/cli/`: redacted REST client and transparent MCP stdio bridge. Keep game
  rules server-side, protocol bytes on stdout, and diagnostics on stderr.
- `lexicons/`: source pins, exact third-party notices, and lexicon pack
  contracts. Never commit downloaded archives, generated packs, or word data.
- `rulesets/`: immutable English/French physical board, premium, tile, scoring,
  and exact lexicon inputs. A changed fixture requires a new ruleset identity.
- `web/`: Vite/React frontend. Shared UI primitives live in
  `web/src/components/ui`; game-specific compositions belong under
  `web/src/components/game` when introduced.
- `docs/`: maintained plans and architecture decisions. Keep the plan in sync
  when a phase begins or finishes.

Add nested `AGENTS.md` files only when a subtree genuinely needs different
commands or conventions. Do not repeat this file in every directory.

## Supported commands

Backend:

```bash
cargo xtask setup
cargo xtask setup --offline
cargo xtask ruleset verify
cargo xtask lexicon audit
cargo xtask lexicon list
cargo xtask lexicon verify
cargo xtask lexicon inspect word-arena-en-world-v1
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --all-features
cargo test -p word-arena-persistence --all-features
cargo run -p word-arena-server
cargo run -p word-arena-cli -- --help
scripts/mcp/verify-contract.sh
scripts/web/verify-contract.sh
```

Frontend:

```bash
bun install --cwd web
bun run --cwd web dev
bun run --cwd web check
bun run --cwd web test
bun run --cwd web format
bun run --cwd web fix
```

Add a shadcn component from `web/` with:

```bash
bunx --bun shadcn@latest add <component>
```

Use Bun only for the frontend. Do not add npm, pnpm, or Yarn lockfiles.

## Domain invariants

- The backend is the authoritative referee. Never trust an agent or browser to
  calculate legality, score, draws, turn order, or game completion.
- Keep state transitions deterministic. Inject the tile source, clock, IDs, and
  other nondeterministic inputs at application boundaries.
- An accepted placement, score update, rack refill, event append, and turn
  advance form one atomic operation. Do not expose a standalone draw action.
- Bind agent identity to an authenticated seat. Never accept a caller-supplied
  player ID as authorization.
- A player may read its own rack and all public state, but never another rack or
  the future bag order. Spectator projections must preserve the same boundary.
- Include `turn_id`, `expected_version`, and an idempotency key in mutating agent
  actions so retries cannot commit twice.
- Represent letters and tiles as normalized string tokens, not Rust `char`s or
  ASCII bytes. Language rules may fold accents or tokenize more than one code
  point.
- Version and hash rulesets, lexicons, RNG algorithms, public schemas, and agent
  manifests. A recorded game must identify every input needed for replay.
- Keep public and seat-private events distinguishable from their creation.
- Never commit proprietary word lists. Every lexicon pack needs source,
  version, locale, license, normalization, and checksum metadata.
- Keep downloaded source archives, compiled pack artifacts, caches, and
  installed pack data outside Git. Use `cargo xtask` for pack lifecycle work.
- Lexicon releases use independent `lexicons-v*` tags and immutable GitHub
  releases. Follow `lexicons/RELEASING.md`; publish a draft with every source,
  license, notice, legible, audit, checksum, and build asset before finalizing.

## Rust conventions

- Use stable Rust 2024 and inherit workspace dependencies and lints.
- Keep `crates/engine` pure and dependency-light. Prefer explicit domain types
  and exhaustive matches over stringly typed state.
- Model transitions as validated input producing a new state plus immutable
  events. Persistence and notifications consume those events outside the engine.
- Avoid `unsafe`. If it ever becomes necessary, document the invariant and get
  explicit project-owner agreement before adding it.
- Use structured `tracing`; do not use `println!` for server diagnostics.
- Add unit tests for rule behavior and property tests for conservation,
  determinism, scoring, and replay invariants as those systems are introduced.
- Keep transport DTOs separate from domain types when their lifecycle diverges.

## Web UI conventions

- Treat the web app as a local operator and game workspace, not as a marketing
  site. The default route should prioritize the active game, seat, or tournament
  state instead of a product landing page.
- Build all general-purpose UI and interactive controls from shadcn/ui
  primitives. Do not introduce another component library.
- Base UI is the sole shadcn primitive layer. Keep every generated component on
  the `base-nova` registry configured in `web/components.json`.
- Add primitives with the shadcn CLI and commit the generated source. Do not
  import `@base-ui/react` directly outside `web/src/components/ui`.
- Compose game-specific board, square, rack, and tile components from shadcn
  primitives, semantic HTML, and shared design tokens; do not fork a parallel
  button, card, dialog, menu, input, table, or tooltip system.
- Use `cn` from `@/lib/utils` and semantic CSS variables from `index.css`.
  Avoid hard-coded theme colors inside components.
- Preserve keyboard navigation, focus states, accessible labels, contrast, and
  reduced-motion behavior. The board must remain inspectable without a canvas.
- Keep server state in a typed API layer rather than duplicating it across
  components. Treat WebSocket events as invalidation/input to authoritative
  snapshots, not as an alternate source of truth.
- Run Biome, TypeScript, and the production build after UI changes.

## MCP and agent conventions

- MCP is a thin authenticated adapter over application use cases, never a
  second implementation of game rules.
- Keep the competitive player tool surface small: observe game, get rules,
  place tiles, exchange, pass, and resign. Preview/solver tools belong only in
  explicitly configured practice modes.
- Return structured content with stable schemas and concise model-readable
  summaries. Annotate read-only and mutating tools accurately.
- Support Streamable HTTP for remote agents and a stdio bridge for local clients.
  Do not couple the engine to any single agent harness or model provider.
- Run autonomous agent processes in isolated seat workspaces with explicit time,
  tool, network, and compute budgets.

## Documentation and dependencies

- Record a short architecture decision before introducing a new datastore,
  queue, transport, agent runtime, or cross-cutting framework.
- Prefer a modular monolith. Do not add Redis, Kafka, or separate services until
  measured load or reliability requirements justify them.
- Add the smallest dependency that solves the problem. Verify its maintenance,
  license, and necessity; keep lockfiles committed.
- Do not manually edit generated output such as `target/`, `web/dist/`, or
  dependency directories.

## Definition of done

- The implementation preserves domain and privacy invariants.
- Relevant tests cover success, invalid input, authorization, and replay paths.
- Rust formatting, Clippy, tests, and build pass.
- Frontend formatting, linting, type checking, and production build pass when
  web files change.
- Public behavior and architecture changes are reflected in `README.md`, the
  project plan, or a focused document.
- Lexicon supply-chain changes pass `cargo xtask lexicon audit` and the focused
  offline commands in `docs/QUALITY_GATES.md`.
- The diff contains no credentials, proprietary lexicons, generated build
  output, unrelated formatting, or accidental lockfiles.
