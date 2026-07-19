# Project quality gates

## Web application

The web job performs a frozen Bun install, runs Biome, TypeScript checks for
application and test code, the unit/component suite, and a production build. It
then installs Chromium and runs deterministic Playwright flows in desktop and
mobile projects. Axe scans the operator, player, spectator, authentication,
and replay states. Browser screenshots and traces use `only-on-failure` and
`retain-on-failure`; CI uploads the failure directory only when the job fails.

`web/fixtures/server.ts` is a self-contained Bun referee for browser tests. It
serves explicit player, spectator, reconnect, replay, terminal, authentication
failure, and privacy scenarios through the V1 REST/WebSocket shapes. It does
not read the database, contact a dictionary provider, or download/build a
lexicon. The CI job finishes by rejecting tracked generated lexicon data and
any dirty output.

Focused commands from the repository root:

```bash
bun run --cwd web test:unit
bun run --cwd web test:e2e -- --project=desktop-chromium --grep "player stages"
```

Install Chromium once, then run the complete clean-install-equivalent web gate:

```bash
bun install --cwd web --frozen-lockfile
bun run --cwd web playwright install chromium
bun run --cwd web check:full
```

## Lexicon V1

The normal Rust job still runs formatting, strict Clippy, the complete workspace
test suite, and an all-features build. The dedicated **Offline lexicon and replay
contract** CI job makes the V1 supply-chain requirements explicit and runs after
one locked dependency fetch with Cargo network access disabled.

| Requirement | Authoritative CI evidence |
| --- | --- |
| Source pins and cross-file consistency | `cargo xtask lexicon audit` parses the committed source and pack registries, loads strict English/French policies, matches revisions/archive hashes/sizes/profiles/licenses, and validates curation governance. |
| License integrity | The audit hashes each committed upstream license and compares it to `sources.toml`; release metadata and required notice/build documents must exist and be nonempty. |
| Pack manifests and checksums | `word-arena-lexicon` contract/runtime tests cover strict schemas, required/unlisted files, per-file and content hashes, incompatible versions, corrupt/truncated FSTs, key normalization, and counts. |
| Reviewed overrides | `word-arena-lexicon-builder` tests reject invalid, duplicate, conflicting, no-op, undocumented, or self-approved curation and verify deterministic reports/changelogs. |
| Deterministic builders | Hand-authored English/French/filter/index tests build twice and compare bytes. The separate Lexicon release workflow performs two complete pinned upstream builds and compares every compiled/source/legible/audit byte before publication. |
| Atomic install and notices | `xtask` local-server tests cover clean setup, idempotency, offline mode, checksums, missing notices, interruptions, unavailable networks, concurrency, CLI metadata, and offline server loading. |
| Offline lookup | Runtime and setup contract tests run with `CARGO_NET_OFFLINE=true`; the engine has no HTTP client or dictionary fallback. |
| Ruleset/replay compatibility | Engine golden tests compare serialized English/French replay state, require registry-matching ruleset pins, reject missing/substituted packs before mutation, and cover cross words plus French blank normalization. |
| No generated data in Git | CI rejects source archives, generated keys/audits under `lexicons/`, tracked/untracked changes, and dirty output after all contract tests. |

Run the same gates locally:

```bash
cargo fetch --locked
CARGO_NET_OFFLINE=true cargo xtask lexicon audit
CARGO_NET_OFFLINE=true cargo test -p word-arena-lexicon --all-features
CARGO_NET_OFFLINE=true cargo test -p word-arena-lexicon-builder --all-features
CARGO_NET_OFFLINE=true cargo test -p word-arena-engine --all-features --test lexicon_games
CARGO_NET_OFFLINE=true cargo test -p xtask --all-features --test local_setup
```

The networked full-corpus reproducibility workflow is intentionally separate
from ordinary pull-request CI because it downloads and redistributes the exact
pinned upstream archives. Run it manually before changing a release pin; tag
publication repeats it before creating an immutable release.

## Agent runtime isolation

The runtime workspace suite validates the tagged V1 layout and credential
contract, private filesystem modes, deterministic resume, retention, config
integrity, path attacks, empty inherited environments, and output redaction.
On macOS or Linux hosts with a supported sandbox it also runs two hostile seat
processes concurrently and requires direct and symlinked cross-seat reads to
fail:

```bash
cargo test -p word-arena-agent-runtime --all-features --test workspace
cargo test -p word-arena-agent-runtime --all-features --test authority
```

Runtime sandbox detection is fail closed. A missing platform sandbox is an
explicit deployment error, not a skipped isolation policy.
The authority suite additionally injects known human-spectator and administrator
tokens into each untyped startup surface, requires a non-disclosing V1 audit,
and proves execution and allocation fail when authority or audit persistence is
unsafe.

## Agent resource budgets

The deterministic budget suite validates the V1 capability/telemetry contract,
strict rejection of weaker platform support, saturating accounting, deadline
races, output floods, semantic attempt/tool limits, and complete process-group
termination:

```bash
cargo test -p word-arena-agent-runtime --all-features --test budget
```

Platform pressure reporting can be repeated explicitly with
`WORD_ARENA_RUN_PLATFORM_BUDGET_SMOKE=1 scripts/agents/smoke-budgets.sh`.
Unenforced CPU, memory, and non-denied network-byte dimensions are a reviewed
capability result, never an implicit pass.

## Agent run telemetry privacy

The capture suite validates the published schema/limits, generated secret
corpora, token and sensitive-key redaction, invalid UTF-8/control handling,
truncation, exact/estimated/unavailable metrics, checked cost arithmetic,
ordering, and structurally content-free public projections:

```bash
cargo test -p word-arena-agent-runtime --all-features --test telemetry
cargo test -p word-arena-persistence --all-features
```

The SQLx suite additionally proves exact tournament-to-turn correlation,
terminal-only writes, restart loading, column drift rejection, and transactional
retention of detailed and budget telemetry.

## Tournament schedule determinism

The application suite locks a golden schedule and property-checks pair
coverage, seat/profile fairness, rotating byes, deterministic bytes, concurrent
assignment uniqueness, and progressive Swiss pairing. The SQLx suite covers
atomic insertion, exact restart loading, lifecycle conflicts, rollback, Swiss
progress, and normalized-row tamper detection:

```bash
cargo test -p word-arena-application --all-features --test tournament
cargo test -p word-arena-persistence --all-features --test tournaments
```

## Durable job claims and leases

The job suites validate canonical bounded payloads, deterministic backoff,
deduplication, fairness, concurrent atomic claims, exact expiry/reclamation,
renewal, fencing, cancellation, all terminal outcomes, handler idempotency, and
database restart:

```bash
cargo test -p word-arena-application --all-features --test job
cargo test -p word-arena-persistence --all-features --test jobs
```

## Scheduler controls and recovery

The scheduler suites validate deterministic integer token refill, four-scope
capacity, provider throttling, simultaneous reservations, pause/resume/drain,
cancellation races, worker death, restart reconstruction, immutable retry
inputs, and exactly-once terminal/downstream identities:

```bash
cargo test -p word-arena-application --all-features --test scheduler
cargo test -p word-arena-persistence --all-features --test scheduler
```

## Scoped rating determinism

The rating suites lock the published Glicko-2 example, volatility convergence,
inactivity and numeric ceilings, ties, fixed-point serialization, pool
isolation, paired-game accounting, transaction rollback, restart idempotency,
normalized-row auditing, and exact full-history rebuilds:

```bash
cargo test -p word-arena-application --all-features --test rating
cargo test -p word-arena-persistence --all-features --test ratings
```

## Statistics rebuild and privacy

The statistics suites cover authoritative fixture derivation, all gameplay and
agent metrics, exact scopes/date windows, stable integer rounding, ties,
estimated/unavailable usage, overflow, duplicate identities, incremental/full
rebuild equivalence, transactional rollback, normalized-row audit, and the
structurally smaller public projection:

```bash
cargo test -p word-arena-application --all-features --test statistics
cargo test -p word-arena-persistence --all-features --test statistics
```

## Export reproducibility and privacy

The export suites reproduce public game state without serialized private
events, lock a golden JSONL record, cover every record schema and audience,
verify record/stream checksums, stream 2,000 bounded records twice with identical
bytes, reject ordering/size/schema/tamper failures, and scan public output for
private keys and credential-shaped values:

```bash
cargo test -p word-arena-engine --all-features --test public_replay
cargo test -p word-arena-application --all-features --test export
```
