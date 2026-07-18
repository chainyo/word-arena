# Lexicon V1 quality gates

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
