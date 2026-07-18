# Offline lexicon V1 completion audit

Audit date: 2026-07-18. Scope: the implementation backlog `LEX-001` through
`LEX-010` in [`TASKS.md`](TASKS.md). German/Spanish source evaluation,
operator-supplied proprietary packs, and signed registry metadata remain
explicit later tasks and are not part of English/French V1 completion.

## Released identities

| Pack | Words | Content SHA-256 | Archive SHA-256 |
| --- | ---: | --- | --- |
| `word-arena-en-world-v1@1.0.0` | 247,450 | `27faaa6b78de526d7e7681bf1af45ce952cb0400897190c79eab7c67b278a54b` | `c3065fbf9850e58158c58c7c6a9cbd448d7656e1477027d1183df4e489bb0839` |
| `word-arena-fr-v1@1.0.0` | 644,894 | `c926a5f1ead63711d041277c9bfb3af23f3a460bb6edf57ff66408552c495193` | `e119c6e8c38af2c96f04b039de5a603b8cbf415e007f864f72db05bb0bed9e31` |

The independent
[`lexicons-v1.0.0`](https://github.com/chainyo/word-arena/releases/tag/lexicons-v1.0.0)
release is published with 21 assets under GitHub release immutability. Its tag,
compiled packs, exact upstream archives, legible keys, row audits, licenses,
notices, curation/build instructions, release metadata, build-materials archive,
and checksums are covered by GitHub's immutable-release attestation. `gh release
verify lexicons-v1.0.0` succeeds.

## Requirement evidence

| Backlog item | Current-state proof |
| --- | --- |
| LEX-001 source/license pins | `lexicons/sources.toml`, exact files in `lexicons/licenses/`, `lexicons/THIRD_PARTY_NOTICES.md`, and the offline repository audit. No proprietary list/checker is an input. |
| LEX-002 pack contract | `crates/lexicon` strict manifest, content identity, normalization, compatibility, and malformed/golden contract tests. |
| LEX-003 English builder | Versioned English policy/importer, source-row audit and filter accounting tests, two-build fixture determinism, plus three full pinned-archive builds (local and CI) matching the released bytes. |
| LEX-004 French builder | Versioned Morphalou policy/streaming importer, active/inactive accounting and orthography tests, two-build fixture determinism, plus three full pinned-archive builds matching released bytes. |
| LEX-005 curation governance | Typed additions/removals/governance, open evidence allowlist, independent review/high-impact approvals, deterministic changelog/report, rejection tests, and disputed-word issue form. |
| LEX-006 runtime index | Deterministic FST compiler; complete outer/index/key/count validation; owned immutable bytes; allocation-conscious exact lookup; corrupt/truncated/unsupported tests with Cargo offline. |
| LEX-007 setup | Workspace `cargo xtask`; platform paths/override; registry; safe extraction; full archive/manifest/license/index checks; atomic/cache/concurrency behavior; lifecycle CLI; local HTTP failure-mode tests; real immutable-release clean install and offline server start. |
| LEX-008 games/replay | Registry-matching static English/French ruleset pins; query-only validator; atomic main/cross validation; pack identity in state/events/snapshots/results/replays; exact create/resume/replay checks; byte-equivalent English/French golden replays and blank/cross tests. |
| LEX-009 release | Tag/workflow-independent data version, two full source builds and all-output byte comparison, deterministic 21-asset packaging, draft-first immutable publication, attestation verification, source/legible/audit/license materials, CLI and health metadata. |
| LEX-010 docs/quality | Setup-first README, provenance/curation/dispute/location/update/rollback/removal/release docs, explicit offline CI contract matrix, prominent non-official-dictionary statement, and generated-data Git audit. |

## Verification commands

The final local and CI contract is defined in [`QUALITY_GATES.md`](QUALITY_GATES.md).
Completion requires all of these classes of evidence, not merely a successful
workspace compile:

- repository audit and tamper tests;
- strict formatting and Clippy;
- complete Rust tests and build;
- focused Cargo-offline lexicon, builder, setup, and replay tests;
- Bun/Biome/TypeScript/Vite checks;
- clean Git status with no generated source/archive/pack data;
- immutable release and release-asset attestation verification.

The real release install was additionally verified in a new temporary data
directory: both registry URLs downloaded, installed, and exposed their
license/source metadata; `setup --offline` succeeded with all proxies directed
to an unavailable endpoint; the server loaded both exact identities and served
health metadata; the temporary process was then shut down.
