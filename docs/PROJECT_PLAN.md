# Word Arena creation plan

This document is the maintained delivery plan for turning the repository into a
reproducible AI-agent word-game arena. Check off work only after its verification
commands pass, and update the plan when a decision materially changes scope.

## Product outcome

Word Arena should let Codex, Claude Code, Cline, Pi, provider-native agents,
custom programs, and humans play the same authoritative game. Operators should
be able to run anything from one local match to large multilingual tournaments,
watch games live, replay them deterministically, and compare agent performance.

The first complete vertical slice is:

1. Create one English game from a versioned ruleset and word-validation policy.
2. Seat two independently authenticated agents.
3. Let both observe and act through MCP until the game finishes.
4. Refill racks atomically after accepted moves.
5. Let a human-only spectator watch the complete current game in the web UI and
   replay it from stored events.
6. Produce a result containing scores, timings, agent manifests, and hashes of
   every reproducibility input, including the recorded word verdicts used while
   refereeing the game.

## Architectural commitments

- Build a modular monolith before considering distributed services.
- Keep a pure, deterministic Rust engine behind all transports.
- Use immutable events plus transactional snapshots for persistence and replay.
- Treat racks as private between competitive seats. Agents and players see only
  their own rack; a distinct human-only spectator projection may show every
  current rack, but no live role sees the future bag order.
- Store rules, board premiums, tile distributions, language normalization, and
  word-validation policy references in immutable versioned packs.
- Keep external HTTP and provider-specific parsing outside the engine. Feed
  recorded word verdicts into deterministic transitions and never re-query a
  provider while replaying a game.
- Use SQLite through SQLx for the first local-first persistence layer.
- Expose REST/WebSocket for the web application and MCP plus a CLI for agents.
- Use Streamable HTTP as the primary remote MCP transport and provide a stdio
  bridge for local agent clients.
- Build the React interface from shadcn/ui Base UI primitives with Tailwind CSS
  4.
- Do not scrape, cache, compile, or redistribute third-party word lists. Keep
  provider integrations replaceable and review their automation terms before
  enabling them by default in a public release.

## Word validation V1 decision

- V1 validates each word formed by a proposed move through a small server-side
  HTTP provider adapter. One game runs at a time, so low request throughput is
  acceptable; this is not the tournament-scale design.
- The initial English candidate is the checker at
  `https://scrabblewordfinder.org/dictionary/{word}`. Its public form posts to
  `/check` and redirects to that result page, which reports the dictionaries in
  which a word is valid.
- This is an HTML checker, not a documented API. Valid and invalid result pages
  both return HTTP 200, so the adapter must recognize explicit valid/invalid
  markers. A timeout, transport failure, rate limit, markup change, ambiguous
  result, or unexpected status means `provider_unavailable`, never
  `word_invalid`.
- The provider base URL, parser version, timeout, and enabled languages are
  configuration. The engine depends only on a validation boundary, while tests
  use a deterministic fake implementation and make no network requests.
- Persist the normalized word, verdict, provider identifier, parser version,
  and observation time with the accepted move. Replay consumes that recorded
  verdict rather than contacting the live website.
- Do not derive or store a local dictionary from provider responses. Before the
  checker becomes a default integration in a public release, confirm automated
  lookup permission and acceptable-use limits with the site owner; its robots
  policy and informal site copy are not a data-reuse license.
- English is the first slice. French, German, and Spanish require separate
  providers or documented locale support behind the same adapter contract.

## Phase 0: repository foundation

- [x] Create the Git repository and Rust 2024 workspace.
- [x] Add a compiling engine crate and Axum server with a health endpoint.
- [x] Scaffold Vite, React 19, Tailwind CSS 4, and current shadcn/ui with Base UI
  primitives.
- [x] Establish Bun as the frontend package manager.
- [x] Add CI, formatting, linting, test, and build commands.
- [x] Add root project guidance, README, license, and this creation plan.

Exit criterion: a clean clone can run the server, render the local game
workspace foundation, and pass every command documented in `AGENTS.md`.

## Phase 1: deterministic engine and rulesets

- [ ] Define typed game, board, square, rack, bag, tile-token, seat, turn, move,
  score, violation, and event models.
- [ ] Implement a locale-aware normalization and tile-tokenization boundary.
- [ ] Define a static ruleset schema and validation command.
- [ ] Add initial English, French, German, and Spanish tile/board fixtures.
- [ ] Implement placement, connectivity, word construction, premiums, blanks,
  bingo bonuses, pass, exchange, resignation, and endgame scoring.
- [ ] Define the deterministic word-validation boundary and recorded verdict
  model; keep HTTP implementations in the application layer.
- [ ] Inject a fixed and versioned RNG algorithm; add seed commitment/reveal.
- [ ] Add golden games, property tests, and small hand-authored validation
  fixtures that do not copy a third-party word list.
- [ ] Build random-legal and greedy baseline bots for engine verification.

Exit criterion: games replay to byte-equivalent public state from the same
ruleset, recorded word verdicts, event stream, and RNG inputs.

## Phase 2: application service and persistence

- [ ] Define application commands and seat-aware queries around the engine.
- [ ] Add SQLite migrations through SQLx for games, seats, events, snapshots,
  rulesets, validation policies/verdicts, tournaments, matches, and agent runs.
- [ ] Append events and update snapshots in one optimistic-concurrency
  transaction using expected game versions.
- [ ] Separate public, per-seat private, human-spectator, and administrator
  projections. Never grant spectator credentials to an agent process.
- [ ] Add capability-style seat tokens, expiry, revocation, and audit logging.
- [ ] Add REST snapshots and WebSocket invalidation/event streams.
- [ ] Add idempotency, turn deadlines, invalid-attempt policy, and recovery tests.
- [ ] Add the configurable English HTTP checker adapter with strict timeouts,
  explicit verdict parsing, minimal rate limiting, and deterministic fake tests.

Exit criterion: concurrent or retried actions cannot double-commit or disclose
private state, and a restarted server resumes active games correctly.

## Phase 3: MCP and universal agent access

- [ ] Add the official Rust MCP SDK and expose authenticated Streamable HTTP.
- [ ] Implement `observe_game`, `get_ruleset`, `play_tiles`, `exchange_tiles`,
  `pass_turn`, and `resign` with versioned input/output schemas.
- [ ] Expose equivalent authenticated resources for public state, private seat
  state, history, rules, and the active word-validation policy.
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
- [ ] Add concurrency and provider-rate controls, cancellation, retry, and worker
  recovery.
- [ ] Compute per-language and per-ruleset Glicko-2 ratings.
- [ ] Report win rate, spread, average move score, bingos, invalid actions,
  passes, exchanges, turn latency, premium use, vocabulary, tool calls, tokens,
  and cost where available.
- [ ] Export complete public replay bundles and analytics-friendly data.

Exit criterion: an operator can run a reproducible paired tournament at a fixed
concurrency and publish its standings and replay bundle.

## Phase 7: multilingual validation and operational hardening

- [ ] Add documented provider adapters for French, German, and Spanish behind
  the same validation contract.
- [ ] Define provider manifests containing provenance, locale, parser version,
  normalization, availability policy, and compatibility metadata.
- [ ] Add provider health checks, bounded retries, circuit breaking, and clear
  operator diagnostics without turning outages into invalid-word verdicts.
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
- Offline lexicon compilation and redistribution unless a future source has
  explicit compatible licensing
- Redistributing proprietary official tournament dictionaries
- Blockchain or public randomness infrastructure beyond commitment/reveal
- Canvas-only board rendering
