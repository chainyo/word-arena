# Word Arena implementation tasks

This is the implementation-ready backlog for the deterministic game-engine
vertical slice. Keep task status aligned with
[`PROJECT_PLAN.md`](PROJECT_PLAN.md), and check an item only after its stated
verification passes. The completed offline lexicon backlog and its evidence are
preserved in [`LEXICON_V1_AUDIT.md`](LEXICON_V1_AUDIT.md).

## Game-engine outcome

Given an immutable ruleset, exact Word Arena lexicon pack, versioned RNG seed,
and two deterministic players, the engine can play a complete English or French
game entirely in memory and replay it to byte-equivalent final state.

The referee is authoritative for tile ownership, bag order, move legality,
scoring, draws, turn order, and completion. A player sees only its own rack and
public state. A human spectator may see both current racks, but no live role sees
the future bag order.

## Tasks

### GAME-001: Define physical game data and versioned rulesets

- [x] Introduce explicit typed models for physical tile tokens, stable tile
  identities, racks, bags, board squares, premiums, seats, turns, moves,
  violations, scores, and game events without coupling the engine to transport
  or persistence.
- [x] Require every physical tile and blank assignment to occupy exactly one
  canonical `A` through `Z` board token; French accented source spellings remain
  playable through normalized multi-tile board forms such as `ÉTÉ -> ETE` and
  `ŒUF -> OEUF`.
- [x] Extend the immutable ruleset schema with board dimensions/premiums, rack
  capacity, bingo bonus, exchange threshold, scoreless-turn limit, tile
  distribution, tile values, language normalization, and exact lexicon
  identity.
- [x] Add complete English and French V1 tile distributions, letter values, and
  classic 15x15 premium-board fixtures. Reject malformed, asymmetric, duplicate,
  incomplete, or arithmetically unsafe definitions.
- [x] Add a deterministic ruleset identity/hash and a validation command that
  verifies every built-in ruleset and its lexicon pin.

Verification:

- Ruleset fixtures round-trip without losing identity and produce stable hashes.
- Unit tests cover tile counts/values, premium symmetry, center square, rack and
  exchange constraints, canonical French tile behavior, and malformed fixtures.
- Existing exact-pack placement and replay tests continue to pass.

### GAME-002: Add deterministic bag randomness and initial deal

- [x] Define and document a fixed, independently versioned pseudo-random
  algorithm and unbiased shuffle procedure; never depend on a platform RNG.
- [x] Represent a game seed as fixed bytes, record a pre-game SHA-256 commitment,
  and support reveal/verification without exposing the seed or bag order during
  live play.
- [x] Construct stable tile identities from the selected ruleset, shuffle once,
  and deal both opening racks deterministically.
- [x] Draw only through an engine-internal bag operation and preserve the exact
  order across snapshots and replay. Do not expose a standalone player draw
  action.
- [x] Enforce tile conservation across bag, racks, and board with duplicate-ID
  detection and checked arithmetic.

Verification:

- Golden seeds produce byte-identical bag orders and opening racks across runs.
- Commitment verification accepts the exact reveal and rejects substitutions.
- Unit/property tests cover empty/partial draws, no replacement, conservation,
  determinism, and distribution sanity for English and French.
- Public serialization contains neither the seed nor future bag order.

### GAME-003: Make tile placement a complete atomic transaction

- [x] Require every placed tile ID to exist in the acting seat's rack and reject
  duplicates, token substitutions, forged blanks, and stale/wrong turns before
  mutation.
- [x] Retain alignment, contiguity, connectivity, main/cross-word construction,
  blank handling, and exact offline lexicon validation for every formed word.
- [x] Apply letter and word premiums only when newly covered, score all cross
  words, and award the configured bingo bonus when the full rack is played.
- [x] Atomically remove tiles from the rack, place them, score the move, refill
  from the bag up to rack capacity, append events, update scoreless state, and
  advance the turn.
- [x] Record enough public and private transition data to verify ownership,
  draws, scoring, and deterministic replay without leaking another rack.

Verification:

- Hand-authored English and French scenarios cover premiums, cross words,
  blanks, bingos, depleted bags, accented input normalization, and multi-tile
  ligature spellings.
- Invalid word, forged tile, occupied square, stale turn, overflow, or failed
  refill validation leaves board, racks, bag, scores, turn, version, and events
  byte-for-byte unchanged.
- Property tests prove tile conservation and score decomposition after every
  accepted placement.

### GAME-004: Implement non-placement actions and endgame scoring

- [ ] Implement pass as an atomic turn action with no tile or score mutation.
- [ ] Implement exchange by validating owned tile IDs and the configured minimum
  bag size, returning exchanged tiles through the deterministic shuffle policy,
  drawing replacements atomically, and preserving conservation.
- [ ] Implement resignation with an explicit terminal reason and immutable
  winner/result.
- [ ] Track consecutive scoreless turns and finish at the configured limit.
- [ ] Finish when a rack becomes empty after the bag is exhausted; subtract
  remaining rack values and award the outgoing player the opponents' deductions
  according to the ruleset.
- [ ] Reject every action after completion and make finish/result events exactly
  replayable.

Verification:

- Unit tests cover legal/illegal exchanges, partial/empty bags, passes,
  zero-score placements, scoreless-limit completion, resignation, empty-rack
  completion, ties, blanks in final racks, and checked score adjustments.
- Scenario tests exercise every terminal reason from creation through replay.
- Failed actions are atomic and every terminal path conserves all tile IDs.

### GAME-005: Add privacy-safe projections, snapshots, and replay

- [ ] Separate authoritative internal state from public, per-seat private,
  human-spectator, and administrator projections.
- [ ] Public state exposes board, scores, turn, bag count, rack counts, ruleset,
  lexicon identity, and completion data, but never rack contents, seed, or future
  bag order.
- [ ] A seat projection exposes only that seat's current rack. A human-spectator
  projection may expose both current racks. No projection exposes future bag
  order, and spectator authority is never representable by an agent seat token.
- [ ] Classify events as public or seat-private at creation; draws and initial
  deals must not leak through public history.
- [ ] Persist authoritative snapshots and replay bundles with schema versions,
  ruleset hash, exact external lexicon-pack identity, RNG algorithm/reveal, and
  all deterministic private events. Replays reference the pack and never embed
  dictionary contents.
- [ ] Reject tampered, missing, reordered, privacy-invalid, or incompatible
  snapshots/events before exposing resumable state.

Verification:

- Serialization tests prove forbidden fields and opponent racks are absent from
  public/seat projections and public events.
- Golden English and French games resume and replay to byte-equivalent internal
  state and every role projection.
- Tamper tests cover rulesets, pack versions, seeds, tile IDs, private draws,
  event ordering, schema versions, and terminal results.

### GAME-006: Add baseline bots and whole-game verification

- [ ] Implement a deterministic random-legal bot for broad engine exploration.
- [ ] Implement a deterministic greedy bot that chooses the highest immediate
  legal score with stable tie-breaking.
- [ ] Keep move generation behind an engine/test boundary and do not expose a
  best-move solver to competitive agent transports.
- [ ] Add complete English and French golden games using small hand-authored
  lexicons plus scenarios against the installed V1 pack boundary.
- [ ] Add property/state-machine tests for determinism, tile conservation,
  ownership, legal turns, scoring, event sequencing, privacy, replay, and every
  terminal reason.
- [ ] Run a deterministic stress suite of at least 1,000 generated complete
  games without panics, nontermination, privacy leakage, conservation failures,
  or invalid terminal states.

Verification:

- The same ruleset, pack, seed, and bot inputs produce byte-identical events,
  projections, and results.
- Random-legal versus random-legal and greedy versus random-legal finish in both
  languages under bounded turns.
- Full workspace formatting, strict Clippy, tests, build, lexicon audit, and CI
  pass with no generated data staged in Git.

## Phase exit criterion

A test-only in-memory match runner can create, deal, play, finish, snapshot,
resume, and replay complete English and French games using only public engine
APIs. Every state transition is deterministic and atomic, every tile is
accounted for, role projections enforce rack privacy, and the exact external
Word Arena lexicon identity remains part of the replay contract.

## Later tasks

- Application commands and SQLite persistence through SQLx.
- Capability-style seat authentication, REST snapshots, and WebSocket updates.
- MCP Streamable HTTP tools and the local stdio bridge.
- Live shadcn web UI backed by authoritative server state.
- Isolated Codex, Claude Code, Cline, Pi, and generic command agent runners.
- Tournament scheduling, ratings, statistics, and export.
- Redistributable German and Spanish Word Arena lexicons.
