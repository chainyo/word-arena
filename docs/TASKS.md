# Word Arena implementation tasks

This is the implementation-ready backlog for every unchecked item in
[`PROJECT_PLAN.md`](PROJECT_PLAN.md). Work in task order unless a documented
dependency requires otherwise. Complete one task on an isolated branch,
exercise its focused and repository-wide quality gates, check both this file and
the matching project-plan item, then commit and push before beginning the next
task.

Phases 0 and 1 are complete. Their engine and lexicon acceptance history is
preserved by the test suite and Git history through GAME-006.

## Delivery rules

- Keep the Rust engine independent from persistence, transport, credentials,
  wall-clock time, and model providers.
- Treat every projection and credential type as an authorization boundary; an
  agent seat must never obtain human-spectator or administrator authority.
- Use SQLite through SQLx before considering a queue or network datastore.
- Use official protocol SDKs and version every externally persisted or exposed
  schema.
- Use shadcn/ui Base UI primitives, Bun, and Biome for the web application.
- Never commit generated lexicon data or redistribute a source without a
  documented license review.
- Every task requires focused tests plus the applicable full commands from
  `AGENTS.md`; transport/UI tasks also require scenario or end-to-end coverage.

## Phase 2 — application service and persistence

### APP-001: Define application commands and seat-aware queries

- [x] Add an application crate between transports and the engine with typed
  create-game and game-action commands carrying game ID, expected version,
  turn identity, and idempotency key.
- [x] Define separate public, seat, human-spectator, and administrator query
  requests/results without caller-supplied authorization roles.
- [x] Inject game repository, lexicon resolver, ID, seed, and clock boundaries;
  do not couple application use cases to Axum or SQLite.
- [x] Cover command routing, wrong-seat authorization, stale versions, missing
  games/packs, projection isolation, and deterministic engine errors with unit
  tests using in-memory adapters.

Verification: application use cases can create, observe, act on, and finish an
English or French game exclusively through typed application APIs, while a seat
cannot request or deserialize another authority's projection.

### APP-002: Add versioned SQLite migrations through SQLx

- [x] Record the SQLite/SQLx persistence decision and add the minimum supported
  SQLx dependency/features with a committed lockfile.
- [x] Add forward-only migrations for games, seats, events, private events,
  snapshots, rulesets, lexicon packs, tournaments, matches, agent manifests,
  agent runs, idempotency records, and audit records.
- [x] Add constraints, foreign keys, indexes, schema-version metadata, UTC time
  representation, and opaque secret-digest storage; never store raw tokens.
- [x] Provide an idempotent migration entry point and temporary-database tests
  for clean install, upgrade ordering, constraints, and rollback-on-failure.

Verification: a fresh SQLite database migrates offline to the expected schema,
all declared relationships reject corrupt rows, and SQLx queries compile in CI.

### APP-003: Persist events and snapshots atomically

- [ ] Implement the SQLx game repository and lexicon/ruleset metadata adapters.
- [ ] Append public/private events and replace the authoritative snapshot in one
  transaction guarded by the expected game version.
- [ ] Distinguish not-found, conflict, incompatible schema/pack, corruption, and
  transient storage failures without leaking private payloads in diagnostics.
- [ ] Prove transaction rollback, concurrent writer conflict, event sequence,
  exact resume, and replay equivalence with integration tests.

Verification: exactly one of two writes at the same expected version commits;
after process reconstruction, stored snapshots/events resume to the same state.

### APP-004: Bind projections to application credentials

- [ ] Introduce unforgeable application credential variants for public viewer,
  competitive seat, human spectator, and administrator.
- [ ] Map each credential variant to exactly one engine projection and prohibit
  role escalation or cross-game/seat reuse.
- [ ] Make human-spectator issuance an operator-only path that is not
  representable in agent-run configuration or seat commands.
- [ ] Add compile-time/type-shape checks and authorization/serialization tests
  covering every allowed and denied role-query pairing.

Verification: no seat-facing API can return opponent racks, future bag order,
seed, administrator snapshot, or spectator projection.

### APP-005: Add capability tokens, expiry, revocation, and audit logging

- [ ] Generate cryptographically strong opaque capabilities, persist only
  versioned keyed digests, and return raw tokens only once at issuance.
- [ ] Bind capabilities to game, role/seat, scopes, issued/expiry times, and an
  optional agent run; inject the clock and token source for deterministic tests.
- [ ] Support rotation and immediate revocation without invalidating unrelated
  seats, and use constant-time digest comparison.
- [ ] Append privacy-safe audit records for issuance, authentication outcome,
  revocation, and privileged access without recording tokens or rack content.

Verification: expired, revoked, malformed, cross-game, cross-seat, and
wrong-scope capabilities fail closed; audit tests prove secrets never serialize.

### APP-006: Add REST snapshots and WebSocket invalidation streams

- [ ] Record the versioned HTTP/WebSocket transport decision and publish strict
  request/response/error schemas for create, observe, rules, and game actions.
- [ ] Authenticate capabilities from headers and derive authority server-side;
  never accept a role or seat as authorization input.
- [ ] Serve role-appropriate snapshots and a reconnectable WebSocket stream of
  public invalidations/version markers, not a competing state store.
- [ ] Add body/connection limits, origin policy, structured tracing, graceful
  shutdown, and Axum integration tests for auth, privacy, reconnect, and errors.

Verification: separate clients observe only their permitted state, reconnect
from a known version, and converge on the authoritative REST snapshot.

### APP-007: Add idempotency, deadlines, invalid-attempt policy, and recovery

- [ ] Make idempotency keys mandatory for mutations and atomically persist the
  exact response/error outcome with command identity and payload hash.
- [ ] Define injected turn deadlines and a deterministic timeout policy that can
  pass or resign a seat exactly once.
- [ ] Persist invalid-attempt counters and a versioned configurable response
  policy without allowing rejected actions to mutate engine state.
- [ ] Add crash-point, restart, duplicate retry, stale retry, timeout race, and
  corrupted-snapshot recovery tests with replay fallback.

Verification: retries never double-commit, timeout/action races have one winner,
and a restarted server recovers every committed game without privacy loss.

## Phase 3 — MCP and universal agent access

### MCP-001: Add authenticated MCP Streamable HTTP

- [ ] Record the MCP transport decision, add the official Rust MCP SDK, and pin
  a supported protocol/version contract.
- [ ] Host Streamable HTTP in the existing server with capability
  authentication, session isolation, request limits, cancellation, and tracing.
- [ ] Expose server metadata/capabilities without game tools until their
  schemas are implemented.
- [ ] Add protocol handshake, authentication, malformed-message, cancellation,
  and graceful-shutdown integration tests.

Verification: an MCP client initializes over authenticated Streamable HTTP and
cannot create or reuse a session with an invalid or different-seat capability.

### MCP-002: Implement competitive game tools

- [ ] Add versioned `observe_game`, `get_ruleset`, `play_tiles`,
  `exchange_tiles`, `pass_turn`, and `resign` input/output schemas.
- [ ] Keep tools as thin adapters over application use cases with accurate
  read-only/destructive/idempotent annotations and concise model-readable text.
- [ ] Require expected version, turn ID, and idempotency key for mutations; bind
  the acting seat exclusively from the authenticated session.
- [ ] Add schema snapshots and successful/invalid/stale/retry/privacy tests for
  every tool in English and French games.

Verification: two authenticated seats can complete and replay a game using only
the competitive MCP tool surface, with no preview or opponent-private access.

### MCP-003: Expose authenticated MCP resources

- [ ] Add stable URI templates for public game, private seat, event history,
  ruleset, and active lexicon manifest resources.
- [ ] Authorize each read from session authority and apply the same projection
  types used by REST; never create MCP-specific game state.
- [ ] Return versioned structured content, MIME types, subscriptions or change
  notifications where supported, and actionable compatibility errors.
- [ ] Test resource listing/reads, URI tampering, cross-game access,
  subscriptions, pack identity, and rack/privacy boundaries.

Verification: MCP resources and tools report one authoritative version and
exact pack/ruleset identities without exposing future bag order or raw secrets.

### MCP-004: Add practice-only move preview

- [ ] Define an explicit practice-game mode at creation that is immutable and
  visible in recorded metadata.
- [ ] Add a rate-limited preview use case and MCP tool that runs authoritative
  validation/scoring without mutation only for practice credentials.
- [ ] Keep move generation and best-move search absent; preview only evaluates
  the caller's supplied placement.
- [ ] Test competitive denial, rate limits, no-state-change behavior, score/error
  equivalence with commit, and audit records.

Verification: no competitive game/session can discover or invoke preview, and
practice preview never changes versions, events, racks, bag, scores, or clocks.

### MCP-005: Build the CLI and stdio bridge

- [ ] Add a `word-arena-cli` crate with configuration precedence for flags,
  environment, and a permission-restricted local config file.
- [ ] Implement health/auth checks, game observation/actions, replay export, and
  a transparent MCP stdio-to-Streamable-HTTP bridge.
- [ ] Keep protocol bytes on stdout and diagnostics on stderr; redact tokens and
  support signals, cancellation, reconnect/backoff, and deterministic exit codes.
- [ ] Add command parsing, redaction, golden JSON, bridge framing, broken-pipe,
  remote error, and local server scenario tests.

Verification: a standard stdio MCP client can play through the bridge without
knowing the HTTP transport, and CLI output never contains another seat's data.

### MCP-006: Validate representative MCP clients

- [ ] Add a reproducible MCP Inspector smoke procedure and checked-in scripted
  clients that exercise initialization, tools, resources, retries, and shutdown.
- [ ] Cover at least the generic stdio bridge and direct Streamable HTTP paths;
  document Codex, Claude Code, Cline, and Pi configuration examples.
- [ ] Run a complete two-client English and French scenario and retain only
  synthetic transcripts/artifacts safe for Git.
- [ ] Add protocol-schema compatibility checks to CI without requiring model
  provider credentials or network access.

Verification: two scripted MCP clients finish and deterministically replay both
language games exclusively through published protocol surfaces.

## Phase 4 — live shadcn web interface

### WEB-001: Establish the typed web application foundation

- [ ] Add generated or hand-maintained typed REST/WebSocket clients with a
  version drift check against server schemas.
- [ ] Add routing, query caching, credential/session storage policy, global
  error boundaries, and reconnect-aware invalidation handling.
- [ ] Keep authoritative server snapshots as the sole game state and avoid
  leaking spectator/seat credentials across routes or browser storage.
- [ ] Add unit tests for decoding, auth failures, cache keys, invalidation,
  reconnect, and error/recovery states.

Verification: the local default route opens the active game workspace, fetches
the correct projection, and recovers from a dropped WebSocket connection.

### WEB-002: Build responsive game components

- [ ] Build board, premium square, physical tile, rack, scores, clocks, and move
  history using shadcn Base UI primitives, semantic HTML, and shared tokens.
- [ ] Support mouse, touch, and keyboard tile placement with blank assignment,
  exchange/pass/resign confirmation, pending state, and authoritative errors.
- [ ] Preserve board coordinates, premium meaning, tile values, focus order,
  contrast, zoom/reflow, and screen-reader labels across desktop and mobile.
- [ ] Add component and interaction tests for all actions and English/French
  physical-letter behavior.

Verification: a player can inspect and submit a full legal turn without a
pointer, and the UI never applies speculative score or draw state as authority.

### WEB-003: Add operator, spectator, replay, and statistics views

- [ ] Add tournament lobby, live human-spectator, private player, replay,
  standings, and agent-detail routes with explicit authority requirements.
- [ ] Render replay controls from recorded public/private artifacts according to
  operator policy, including seed reveal and exact input identities post-game.
- [ ] Add filters, pagination/virtualization where measured, empty/loading/error
  states, and share/export actions that contain public data only.
- [ ] Test route authorization, opponent rack isolation, replay stepping,
  statistics formatting, and public export privacy.

Verification: a human operator can create, watch, inspect, replay, and export a
game while an agent-seat route remains limited to its own projection.

### WEB-004: Harden accessibility, reconnect, themes, and mobile behavior

- [ ] Add accessible board narration and move summaries, landmarks, skip links,
  focus restoration, live-region discipline, and reduced-motion alternatives.
- [ ] Add light/dark/system themes using semantic tokens without hard-coded
  component colors.
- [ ] Implement offline/reconnecting/stale-session UX, retry/backoff, credential
  expiry/revocation handling, and conflict recovery from fresh snapshots.
- [ ] Verify supported mobile/desktop breakpoints, touch targets, overflow,
  high zoom, keyboard-only use, and no-motion mode.

Verification: automated accessibility checks pass and the core play/spectate
flows remain usable at 320 CSS pixels, 200% zoom, and keyboard-only input.

### WEB-005: Add frontend unit, accessibility, and end-to-end gates

- [ ] Add a Bun-compatible component/unit test runner, DOM testing utilities,
  axe checks, and Playwright end-to-end tests without introducing ESLint.
- [ ] Provide deterministic server fixtures for player, spectator, reconnect,
  replay, terminal game, auth failure, and privacy scenarios.
- [ ] Run critical desktop/mobile flows in CI with screenshots/traces retained
  only on failure and no downloaded/generated lexicon data committed.
- [ ] Document focused and full web verification commands in `AGENTS.md` and
  quality-gate documentation.

Verification: Biome, TypeScript, production build, unit/component,
accessibility, and end-to-end suites pass from a clean Bun install.

## Phase 5 — agent harnesses and sandboxing

### RUN-001: Define versioned agent manifests

- [ ] Define a strict versioned manifest for harness, model, prompt hash, tool
  policy, environment image, driver version, workspace policy, and budgets.
- [ ] Normalize and hash the canonical manifest; validate semantic versions,
  digests, command arguments, and mutually exclusive provider settings.
- [ ] Persist the exact manifest identity with each run/result/replay without
  storing provider secrets.
- [ ] Add schema round-trip, unknown-field, unsafe command, drift, and golden
  identity tests plus documented examples for every supported harness.

Verification: every run is reproducibly attributable to one immutable manifest
and malformed or secret-bearing manifests fail before a process starts.

### RUN-002: Implement the process-driver contract

- [ ] Define async start, request-turn, resume, terminate, and telemetry methods
  with typed lifecycle states, cancellation, and injected process/time adapters.
- [ ] Implement the generic command driver first with stdout/stderr framing,
  structured diagnostics, exit mapping, and crash recovery.
- [ ] Ensure only visible outputs/tool calls are recorded and never request or
  persist hidden chain-of-thought.
- [ ] Add fake-process state-machine tests for every transition, signal race,
  partial output, crash, resume, and idempotent termination.

Verification: the application can drive a synthetic agent through a complete
match and reconstruct its lifecycle/telemetry after restart.

### RUN-003: Add supported harness integrations

- [ ] Add Codex, Claude Code, Cline, and Pi adapters plus the generic command
  adapter behind the common driver contract.
- [ ] Pin documented minimum versions and translate manifest/tool/workspace
  settings without provider-specific behavior entering game rules.
- [ ] Detect unavailable or incompatible executables with actionable errors and
  redact environment/config secrets from commands, logs, and telemetry.
- [ ] Add offline fake-binary contract tests and opt-in local smoke scripts for
  every adapter without requiring paid credentials in CI.

Verification: each harness completes the same scripted turn lifecycle and emits
normalized telemetry through one application interface.

### RUN-004: Isolate persistent seat workspaces and credentials

- [ ] Allocate one explicit non-overlapping workspace per run/seat with safe
  ownership, permissions, path validation, and configurable retention.
- [ ] Inject only that seat's short-lived capability and MCP/CLI configuration;
  never place opponent, spectator, administrator, or database credentials in an
  agent-visible filesystem/environment.
- [ ] Preserve allowed workspace state across turns/resume while preventing
  symlink, traversal, inherited environment, and cross-seat access.
- [ ] Add filesystem/process isolation tests, crash cleanup, retention, and
  adversarial path/environment scenarios.

Verification: two concurrent hostile fixture agents cannot read or infer one
another's workspace, capability, rack, transcript, or operator configuration.

### RUN-005: Enforce human-spectator credential separation

- [ ] Make agent run manifests/configuration unable to name or request
  human-spectator/admin credentials.
- [ ] Keep operator credential issuance and storage outside agent driver state,
  inherited environment, child process arguments, and workspace files.
- [ ] Add startup assertions and audit events for forbidden authority presence,
  failing closed before process execution.
- [ ] Add type-level, serialization, environment, process-argument, and
  recursive-workspace scanning tests.

Verification: injecting any spectator/admin secret into an agent boundary is
detected, audited without disclosure, and prevents the run from starting.

### RUN-006: Enforce resource and tool budgets

- [ ] Define versioned wall-clock, CPU, memory, network, token, attempt, tool,
  output, and cost budgets with platform capability reporting.
- [ ] Enforce hard limits where supported and fail closed or explicitly mark
  unenforced dimensions according to operator policy.
- [ ] Cancel/terminate complete process trees on limit, timeout, game end, or
  operator cancellation and persist normalized limit telemetry.
- [ ] Add deterministic limit/race tests and opt-in platform integration tests
  for process trees, memory/CPU pressure, output floods, and network policy.

Verification: runaway fixture agents terminate within bounded tolerance and no
post-termination process retains credentials or keeps emitting output.

### RUN-007: Record privacy-safe run telemetry

- [ ] Persist visible transcripts, MCP/tool calls, timings, usage, retries,
  failures, and cost when exposed, with versioned schemas and source labels.
- [ ] Apply size limits, secret/token redaction, binary/control-character
  handling, retention policy, and explicit unavailable/estimated markers.
- [ ] Correlate telemetry to tournament/match/game/run/turn without placing
  private rack content in public analytics or exports.
- [ ] Add redaction fuzz/property tests, truncation, cost arithmetic, ordering,
  restart, retention, and public-export privacy tests.

Verification: synthetic secret corpora never appear in stored/logged/exported
telemetry, while all visible reproducibility and performance fields remain.

## Phase 6 — tournaments and statistics

### TOUR-001: Add deterministic tournament formats

- [ ] Model versioned round-robin, paired seat-swap, Swiss, and configurable
  series formats with explicit entrants, languages, rulesets, seeds, and rounds.
- [ ] Generate schedules deterministically with stable tie-breaks, byes,
  rematch/seat-balance policy, and immutable format identity.
- [ ] Persist tournament, series, match, seat assignment, and lifecycle state
  through application repositories.
- [ ] Add golden schedules and property tests for pair coverage, seat balance,
  determinism, no duplicate simultaneous assignment, and Swiss progression.

Verification: identical tournament inputs produce byte-identical schedules and
paired formats give each entrant equal seat/language exposure where possible.

### TOUR-002: Build the SQLx job claim/lease loop

- [ ] Add durable jobs with kind, payload schema, priority, availability,
  attempt, owner, lease expiry, cancellation, and deduplication identity.
- [ ] Claim jobs atomically in SQLite without a network queue, renew leases, and
  recover abandoned jobs after injected-clock expiry.
- [ ] Make handlers idempotent and separate retryable, permanent, cancelled,
  and exhausted outcomes with bounded backoff.
- [ ] Add concurrent worker, crash, lease race, renewal, duplicate enqueue,
  fairness, and database restart integration tests.

Verification: multiple workers never execute one live lease concurrently and a
crashed worker's job becomes safely claimable exactly after lease expiry.

### TOUR-003: Add scheduling controls and worker recovery

- [ ] Enforce global, tournament, harness, and model-provider concurrency/rate
  limits with injected monotonic time and persisted reservation state.
- [ ] Support cancellation propagation, bounded retries, pause/resume, graceful
  drain, and restart reconstruction for tournaments, matches, and agent runs.
- [ ] Prevent retry from changing immutable game inputs or duplicating results,
  charges, telemetry, or ratings updates.
- [ ] Add deterministic token-bucket/queue tests, cancellation races, provider
  throttling, worker death, restart, and fairness scenarios.

Verification: fixed-concurrency tournaments recover from worker crashes and
finish with exactly one terminal result per scheduled match.

### TOUR-004: Compute scoped Glicko-2 ratings

- [ ] Implement tested Glicko-2 periods, volatility iteration, inactivity, and
  numeric bounds without nondeterministic floating serialization.
- [ ] Scope ratings by entrant, language, ruleset, and rated-format policy;
  paired seat swaps must enter the configured rating period exactly once each.
- [ ] Persist immutable rating inputs and versioned derived updates so ratings
  can be rebuilt and audited from match results.
- [ ] Add published-example vectors, convergence/boundary tests, deterministic
  rebuilds, ties, inactivity, and transaction/idempotency tests.

Verification: rebuilding every rating period from immutable results yields the
same versioned ratings and published reference vectors pass within tolerance.

### TOUR-005: Compute match and agent statistics

- [ ] Derive win rate, spread, average move score, bingos, invalid actions,
  passes, exchanges, turn latency, premium use, vocabulary, tool calls, tokens,
  and cost from authoritative events and normalized telemetry.
- [ ] Scope and version aggregations by language, ruleset, pack, agent manifest,
  tournament, seat, and date window with explicit missing-data semantics.
- [ ] Keep private racks/transcripts and unpublished word usage out of public
  aggregates while retaining authorized operator drill-down.
- [ ] Add fixture aggregates, incremental/full rebuild equivalence, ties,
  null/estimated usage, overflow, deduplication, and privacy tests.

Verification: statistics rebuild exactly from source records and public output
contains only policy-approved fields with stable rounding/ordering.

### TOUR-006: Export replays and analytics data

- [ ] Define versioned public replay, tournament result, standings, rating, and
  analytics export schemas with content type, checksums, and provenance.
- [ ] Stream bounded JSON/JSONL or documented archival output in deterministic
  order without embedding lexicon contents, private capabilities, racks, or
  nonpublic transcripts.
- [ ] Support operator-authorized complete exports separately from public
  exports with explicit policy and redaction metadata.
- [ ] Add golden files, schema compatibility, large streaming, checksum,
  deterministic rebuild, and secret/privacy tests.

Verification: an exported paired tournament can be independently checked and
its public replay bundles reproduce every game using referenced immutable packs.

## Phase 7 — multilingual lexicons and operational hardening

### OPS-001: Select German and Spanish lexicon sources

- [ ] Research source candidates from primary license/provenance documents and
  select only sources that explicitly permit required modification and
  redistribution of generated word data.
- [ ] Record immutable revisions, archive checksums, retrieval URLs, SPDX or
  LicenseRef terms, exact notices, attribution, and compatibility analysis.
- [ ] Define inclusion/exclusion and inflection policies with native-speaker
  review requirements before importing any word data.
- [ ] Add repository-audit tests that fail on missing/changed license, notice,
  pin, policy, or reviewer evidence.

Verification: a clean legal/provenance review proves each selected source may be
used for redistributable Word Arena packs; otherwise the language remains gated.

### OPS-002: Build German and Spanish Word Arena packs

- [ ] Add deterministic importers, normalization profiles, curation overrides,
  manifests, notices, and compact exact-membership builds for approved sources.
- [ ] Add immutable physical ruleset fixtures, distributions, board/scoring
  identities, and exact pack pins for German and Spanish.
- [ ] Extend setup/install/verify/audit/build/release tooling and release-asset
  metadata without committing archives, generated packs, or word lists.
- [ ] Add clean-build byte equivalence, policy boundaries, exact lookup,
  corruption, compatibility, game, replay, and licensing tests.

Verification: fresh approved-source builds reproduce installed German/Spanish
packs byte-for-byte and complete offline games replay under exact identities.

### OPS-003: Add pack lifecycle and diagnostics

- [ ] Add explicit list/update/rollback/cache-inspect/verify/repair commands with
  locking, atomic publication, checksums, licenses, and dry-run behavior.
- [ ] Preserve every pack referenced by an active game or retained replay and
  reject destructive rollback/removal while references exist.
- [ ] Produce actionable compatibility diagnostics across ruleset, game,
  replay, installed pack, platform, and normalization versions without network
  lookup during gameplay.
- [ ] Add interruption, concurrent command, corrupt cache/install, unavailable
  network, rollback, reference protection, repair, and offline tests.

Verification: operators recover from corrupt or incompatible installations
without mutating active-game identities or losing the last valid pack.

### OPS-004: Verify four-language orthography and physical tiles

- [ ] Add native-speaker-reviewed fixtures for English, French, German, and
  Spanish accents, inflections, blanks, digraph/ligature boundaries, and every
  physical tile token.
- [ ] Prove board display, placed physical tiles, normalized lookup keys, source
  spellings, and replay serialization obey each language contract.
- [ ] Add cross-language rejection and exact single-pack-per-game scenarios;
  never merge or fall back between language packs.
- [ ] Record reviewer, date, rationale, and source-policy linkage for each
  orthography fixture without copying proprietary dictionaries.

Verification: reviewed golden games cover each normalization/tokenization edge
and replay byte-equivalently with only canonical physical board letters.

### OPS-005: Add operational resilience and security gates

- [ ] Record and implement backup/restore, retention, structured observability,
  health/readiness, graceful shutdown, and migration/recovery procedures.
- [ ] Add representative load/stress tests for games, WebSockets, MCP sessions,
  workers, and exports with documented local resource targets.
- [ ] Add abuse controls, request/output limits, credential rotation, dependency
  and secret scanning, threat model, security checklist, and incident runbook.
- [ ] Add restore drills, retention tests, failure injection, load baselines,
  privacy/log assertions, and deployment configuration validation to CI or
  documented scheduled gates.

Verification: a clean environment can deploy, back up, restore, upgrade, load
test, rotate credentials, and shut down without losing committed games or
disclosing private state.

### OPS-006: Document the stable public protocol and compatibility policy

- [ ] Publish versioned REST, WebSocket, MCP, CLI, replay, export, manifest,
  error, authentication, and deprecation contracts from authoritative schemas.
- [ ] Define semantic compatibility, supported version windows, feature
  negotiation, migration, retention, security reporting, and end-of-life policy.
- [ ] Add schema diff/breaking-change checks, examples for every role and
  language, upgrade paths, and a release checklist tied to CI artifacts.
- [ ] Clearly distinguish Word Arena rules/lexicons from proprietary tournament
  products and document operator-supplied pack responsibilities.

Verification: an independent client can implement the stable public protocol,
authenticate safely, play/watch/replay/export games, and predict compatibility
behavior across supported upgrades.

## Project exit criterion

Every checkbox in this file and `PROJECT_PLAN.md` is complete. A clean install
can run a reproducible multilingual tournament between supported isolated agent
harnesses, let authorized humans operate and spectate it through the accessible
web application, replay/export every result, rebuild ratings/statistics, and
survive restart/backup/upgrade without leaking private state or relying on a
network dictionary. All documented local and hosted quality gates pass.
