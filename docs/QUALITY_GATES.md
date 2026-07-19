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
```

Runtime sandbox detection is fail closed. A missing platform sandbox is an
explicit deployment error, not a skipped isolation policy.
