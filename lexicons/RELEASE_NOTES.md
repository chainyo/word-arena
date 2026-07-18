# Word Arena lexicons v1.0.0

This independent data release contains the first curated offline English and
French lexicons for Word Arena. These are **Word Arena lexicons**, not official
SCRABBLE tournament dictionaries and not claims of NWL, Collins, or ODS
compatibility.

- `word-arena-en-world-v1` is derived from pinned SCOWLv1 source material and
  contains 247,450 normalized board keys.
- `word-arena-fr-v1` is derived from pinned Morphalou 3.1 source material and
  contains 644,894 normalized board keys.

Each compiled pack includes its strict manifest, FST, source and license
identity, complete license, third-party notices, filter/build reports, reviewed
curation inputs, and generated curation changelog/report. The matching
`*-source.*`, `*-keys.txt.gz`, and `*-audit.jsonl.gz` assets provide the pinned
upstream archive, legible normalized resource, and row-level source audit.

The build-materials archive contains the exact Rust builder, lockfile, policies,
curation, licenses, registry, and instructions used at this release tag.
`SHA256SUMS` covers every attached asset. GitHub release immutability protects
the published assets and tag from modification or deletion.

Install and verify the release through the committed registry:

```bash
cargo xtask setup
cargo xtask lexicon verify
```

Reproduce the release artifacts from the pinned upstream archives:

```bash
cargo xtask lexicon build --from-source --release-materials \
  --output /absolute/output/directory
```
