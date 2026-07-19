# Word Arena creation plan

This document is the maintained delivery plan for turning the repository into a
reproducible AI-agent word-game arena. Check off work only after its verification
commands pass, and update the plan when a decision materially changes scope. The
implementation-ready backlog lives in [`TASKS.md`](TASKS.md).

## Product outcome

Word Arena should let Codex, Claude Code, Cline, Pi, provider-native agents,
custom programs, and humans play the same authoritative game. Operators should
be able to run anything from one local match to large multilingual tournaments,
watch games live, replay them deterministically, and compare agent performance.

The first complete vertical slice is:

1. Install the pinned English and French lexicon packs with the supported local
   bootstrap command.
2. Create one game in either language from a versioned ruleset and lexicon pack.
3. Seat two independently authenticated agents.
4. Let both observe and act through MCP until the game finishes.
5. Refill racks atomically after accepted moves.
6. Let a human-only spectator watch the complete current game in the web UI and
   replay it from stored events.
7. Produce a result containing scores, timings, agent manifests, and hashes of
   every reproducibility input, including the exact lexicon pack.

## Architectural commitments

- Build a modular monolith before considering distributed services.
- Keep a pure, deterministic Rust engine behind all transports.
- Use immutable events plus transactional snapshots for persistence and replay.
- Treat racks as private between competitive seats. Agents and players see only
  their own rack; a distinct human-only spectator projection may show every
  current rack, but no live role sees the future bag order.
- Store rules, board premiums, tile distributions, language normalization, and
  lexicon references in immutable versioned packs.
- Inject an exact-membership lexicon boundary into the engine. Competitive word
  validation must be offline, deterministic, and pinned by pack checksum; there
  is no live HTTP fallback.
- Build first-party English and French packs only from sources whose licenses
  explicitly allow the required use and redistribution. Keep pack licenses and
  notices separate from the MIT-licensed application code.
- Make the supported first-time local setup command install frontend
  dependencies and download, verify, and install any missing lexicon packs.
  Once setup succeeds, normal development and gameplay must work without
  network access to a dictionary service.
- Use SQLite through SQLx for the first local-first persistence layer.
- Expose REST/WebSocket for the web application and MCP plus a CLI for agents.
- Use Streamable HTTP as the primary remote MCP transport and provide a stdio
  bridge for local agent clients.
- Build the React interface from shadcn/ui Base UI primitives with Tailwind CSS
  4.
- Never scrape checker websites or copy proprietary official tournament lists.
  Every distributed data pack must include source, version, license,
  normalization, curation policy, build version, and checksum metadata.

## Offline lexicon V1 decision

- Ship `word-arena-en-world-v1`, generated from a pinned SCOWLv1 revision. Use
  normal-word categories only, include the explicitly selected American and
  British spelling profiles through size 80, and exclude names, abbreviations,
  contractions, affixes, and special lists.
- Ship `word-arena-fr-v1`, generated from pinned Morphalou 3.1 data. Use standard
  single-token inflected forms and exclude proper names, abbreviations,
  locutions, explicitly nonstandard forms, punctuation, and unsupported tokens.
- Define deterministic normalization per pack. English uses the configured
  board alphabet. French preserves source forms for audit while its board key
  folds accents and supported ligatures according to the ruleset.
- Keep reviewed additions and removals in small source-controlled override files.
  Each override needs a reason, an openly usable supporting source, reviewer,
  and date. Never derive overrides by comparing against NWL, Collins, or ODS.
- Compile sorted normalized words to a compact exact-membership format suitable
  for memory mapping. The pack contains its manifest, license, source notice,
  curation files, build metadata, word count, and content checksum.
- Distribute lexicon packs as separately licensed release artifacts. The default
  setup path downloads prebuilt pinned packs and verifies committed checksums;
  a reproducible source-build path downloads the pinned upstream archives.
- The canonical first-time workflow will be `cargo xtask setup`. It must be
  idempotent, install only missing data, fail safely on checksum or license-file
  mismatch, and provide an offline verification mode for already cached packs.
- These are Word Arena lexicons, not official SCRABBLE dictionaries and not
  claims of NWL, Collins, or ODS compatibility. Separately licensed
  operator-supplied packs may be supported later behind the same interface.

## Phase 0: repository foundation

- [x] Create the Git repository and Rust 2024 workspace.
- [x] Add a compiling engine crate and Axum server with a health endpoint.
- [x] Scaffold Vite, React 19, Tailwind CSS 4, and current shadcn/ui with Base UI
  primitives.
- [x] Establish Bun as the frontend package manager.
- [x] Add CI, formatting, linting, test, and build commands.
- [x] Add root project guidance, README, license, creation plan, and task
  backlog.

Exit criterion: a clean clone can run the server, render the local game
workspace foundation, and pass every command documented in `AGENTS.md`.

## Phase 1: deterministic engine and rulesets

- [x] Define typed game, board, square, rack, bag, tile-token, seat, turn, move,
  score, violation, and event models.
- [x] Implement a locale-aware normalization and physical tile-tokenization
  boundary for the available English and French rulesets.
- [x] Define a static ruleset schema and validation command.
- [x] Add initial English and French tile/board fixtures. German and Spanish
  remain Phase 7 work alongside their separately reviewed lexicon packs.
- [x] Implement placement, connectivity, word construction, premiums, blanks,
  bingo bonuses, pass, exchange, resignation, and endgame scoring.
- [x] Define the deterministic exact-membership lexicon boundary and immutable
  pack manifest model.
- [x] Add the reproducible English and French pack builders, reviewed curation
  overrides, and compact runtime format.
- [x] Add `cargo xtask setup` to install Bun dependencies and download, verify,
  and install missing English and French packs during first-time local setup.
- [x] Inject a fixed and versioned RNG algorithm; add seed commitment/reveal.
- [ ] Add golden games, property tests, and small hand-authored validation
  fixtures that do not copy a third-party word list.
- [ ] Build random-legal and greedy baseline bots for engine verification.

Exit criterion: games replay to byte-equivalent public state from the same
ruleset, lexicon pack, event stream, and RNG inputs; both V1 packs validate words
without network access after setup.

## Phase 2: application service and persistence

- [ ] Define application commands and seat-aware queries around the engine.
- [ ] Add SQLite migrations through SQLx for games, seats, events, snapshots,
  rulesets, lexicon pack metadata, tournaments, matches, and agent runs.
- [ ] Append events and update snapshots in one optimistic-concurrency
  transaction using expected game versions.
- [ ] Wire the completed engine public, per-seat private, human-spectator, and
  administrator projections to separate application credential types. Never
  grant spectator credentials to an agent process.
- [ ] Add capability-style seat tokens, expiry, revocation, and audit logging.
- [ ] Add REST snapshots and WebSocket invalidation/event streams.
- [ ] Add idempotency, turn deadlines, invalid-attempt policy, and recovery tests.

Exit criterion: concurrent or retried actions cannot double-commit or disclose
private state, and a restarted server resumes active games correctly.

## Phase 3: MCP and universal agent access

- [ ] Add the official Rust MCP SDK and expose authenticated Streamable HTTP.
- [ ] Implement `observe_game`, `get_ruleset`, `play_tiles`, `exchange_tiles`,
  `pass_turn`, and `resign` with versioned input/output schemas.
- [ ] Expose equivalent authenticated resources for public state, private seat
  state, history, rules, and the active lexicon manifest.
- [ ] Add a practice-only, rate-limited move preview capability.
- [ ] Build `word-arena-cli` and an MCP stdio-to-HTTP bridge.
- [ ] Test schemas and behavior with the MCP Inspector and representative agent
  clients.

Exit criterion: two scripted MCP clients finish and replay a full game without
using internal APIs.

## Phase 4: live shadcn web interface

- [ ] Establish typed API clients, query caching, routing, and error boundaries.
- [ ] Build responsive board, square, tile, rack, score, clock, and move-history
  components using shadcn primitives and semantic design tokens.
- [ ] Add tournament lobby, live spectator, private player, replay, standings,
  and agent-detail views.
- [ ] Add reconnect behavior, keyboard controls, accessible board narration,
  reduced motion, light/dark themes, and mobile layouts.
- [ ] Add component, interaction, accessibility, and end-to-end tests.

Exit criterion: a user can create, watch, inspect, and replay the vertical-slice
game without using the command line.

## Phase 5: agent harnesses and sandboxing

- [ ] Define a versioned agent manifest: harness, model, prompt hash, tool policy,
  environment image, budgets, and driver version.
- [ ] Implement a process-driver contract for start, request turn, resume,
  terminate, and telemetry collection.
- [ ] Add Codex, Claude Code, Cline, and Pi integrations plus a generic command
  adapter.
- [ ] Give every seat an isolated persistent workspace and separate credentials.
- [ ] Keep human-spectator credentials outside every agent workspace and process
  environment.
- [ ] Enforce wall-clock, CPU, memory, network, token, attempt, and tool budgets.
- [ ] Record visible transcripts, tool calls, timings, usage, failures, and cost
  when exposed; never depend on hidden chain-of-thought.

Exit criterion: each supported harness can play under the same tool and budget
policy without accessing its opponent's workspace or credentials.

## Phase 6: tournaments and statistics

- [ ] Add round-robin, paired seat-swap, Swiss, and configurable series formats.
- [ ] Build a SQLite-backed SQLx job claim/lease loop before adding a queue.
- [ ] Add concurrency and model-provider-rate controls, cancellation, retry, and
  worker recovery.
- [ ] Compute per-language and per-ruleset Glicko-2 ratings.
- [ ] Report win rate, spread, average move score, bingos, invalid actions,
  passes, exchanges, turn latency, premium use, vocabulary, tool calls, tokens,
  and cost where available.
- [ ] Export complete public replay bundles and analytics-friendly data.

Exit criterion: an operator can run a reproducible paired tournament at a fixed
concurrency and publish its standings and replay bundle.

## Phase 7: additional multilingual lexicons and operational hardening

- [ ] Select explicitly redistributable German and Spanish source lexicons and
  document their license obligations before importing data.
- [ ] Build German and Spanish packs behind the same exact-membership contract,
  manifest schema, installer, and release process.
- [ ] Add pack update, rollback, cache inspection, integrity repair, and
  compatibility diagnostics without changing packs for active games.
- [ ] Test orthography, accents, blanks, and tokenization for all four initial
  languages with native-speaker-reviewed fixtures.
- [ ] Add backup/restore, retention, observability, load testing, abuse limits,
  security review, and deployment documentation.
- [ ] Document a stable public protocol and compatibility policy.

Exit criterion: the project can operate public multilingual tournaments without
licensing ambiguity, privacy regressions, or unbounded infrastructure growth.

## Deferred until justified

- Redis, Kafka, NATS, or a separate queue service
- Splitting the modular monolith into network services
- A server-provided best-move solver in competitive modes
- Scraping online word lists or building a project-owned dictionary from
  third-party checker responses
- Redistributing proprietary official tournament dictionaries
- Claiming exact NWL, Collins, or ODS compatibility without a separately
  licensed operator-supplied pack
- Blockchain or public randomness infrastructure beyond commitment/reveal
- Canvas-only board rendering
