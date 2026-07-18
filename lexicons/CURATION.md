# Lexicon curation governance

Generated source data is the default authority for each Word Arena lexicon.
Small, exceptional corrections live in the typed files under `curation/`; they
must never be used to recreate or approximate a proprietary tournament list.
The committed V1 files are intentionally empty until a correction has open,
reviewable evidence.

Each pack directory contains exactly these inputs:

- `additions.toml`: keys absent from the generated source set that become
  playable;
- `removals.toml`: generated keys that become unplayable;
- `governance.toml`: versions and independent approvals for broad filter or
  normalization changes.

The builder parses all files with unknown-field rejection. Pack IDs and
normalization profiles must agree across the three documents. An override must
not be duplicated, appear in both actions, add an existing key, or remove an
absent key.

## Override format

Add one table to the appropriate file:

```toml
[[overrides]]
normalized_word = "EXAMPLE"
action = "add"
reason = "Concise explanation of why the generated source set needs correction."
supporting_source_title = "Title of the open linguistic source"
supporting_source_url = "https://example.org/stable-evidence"
supporting_source_license = "CC-BY-4.0"
author = "GitHub identity of proposer"
reviewer = "Different GitHub identity"
date = "2026-07-17"
```

`normalized_word` is the exact uppercase board key, not the display spelling.
It must normalize to itself under the pack profile and contain 2 through 15
tokens. `action` must be `add` in `additions.toml` and `remove` in
`removals.toml`. Every field is mandatory. The author and reviewer must be two
distinct people for every override; this strictly enforces the required
two-person review for disproportionately impactful two-letter words.

Evidence must be reachable through HTTPS and explicitly reusable under one of
these identifiers: `CC0-1.0`, `CC-BY-4.0`, `CC-BY-SA-4.0`, `ODC-BY-1.0`,
`ODbL-1.0`, `PDDL-1.0`, `LicenseRef-LGPLLR`, or
`LicenseRef-SCOWL-v1`. A result from a word-checker website is not evidence of
permission to redistribute. Do not submit copied or derived entries from NWL,
Collins, ODS, OSPD, scrabblewordfinder.org, screenshots, or proprietary lists.

## Dispute process

1. Open the repository's **Disputed playable word** issue form and select the
   affected pack and action. Do not paste a proprietary list entry, comparison,
   screenshot, or checker result.
2. Provide an openly reusable linguistic source, stable HTTPS URL, license, and
   an explanation of how the generated source policy produced the disputed
   result.
3. A maintainer reproduces the current generated result from the pinned source
   and decides whether the source/filter policy or a narrow override is the
   appropriate fix.
4. A pull request adds the typed override or versioned policy change. A second
   person reviews it; high-impact changes also require the matching governance
   approval.
5. CI validates the evidence fields, normalization, non-conflict/no-op rules,
   checksums, and deterministic changelog. The change becomes playable only in
   a new independently versioned immutable pack release.

Closing an issue as unsupported does not assert that a word is linguistically
right or wrong; it means the proposal did not meet the open, reproducible, and
redistributable evidence contract for a Word Arena pack.

## High-impact changes

The initial filter and normalization contracts are the reviewed V1 baseline.
Any subsequent policy or normalization version requires a matching approval in
`governance.toml`:

```toml
[[approvals]]
kind = "broad_filter"
version = 2
summary = "Describe the source-selection or broad-filter change."
tracking_url = "https://github.com/chainyo/word-arena/pull/123"
author = "GitHub identity of proposer"
reviewer = "Different GitHub identity"
date = "2026-07-17"
```

Use `kind = "normalization"` for changes to board-key normalization. Authors
cannot approve their own changes. Pack assembly also cross-checks these
governance versions against the selected builder policy and pack manifest.

## Apply and audit

After generating a sorted `keys.txt`, apply its pack curation into a new output
directory:

```bash
cargo run -p word-arena-lexicon-builder -- \
  curation-apply <generated-keys.txt> <new-output-dir> \
  lexicons/curation/en-world-v1
```

The stage publishes atomically and never overwrites an output directory. It
emits curated `keys.txt`, `curation-changelog.md`, and
`curation-report.toml`. The changelog lists every added and removed key with its
reason, source, license, author, reviewer, and date. The report binds the exact
base keys, three curation documents, curated keys, and changelog by SHA-256, so
every released word-set change is attributable and reproducible.

Compile the resulting keys into the runtime pack payload with:

```bash
cargo run -p word-arena-lexicon-builder -- \
  index-compile <curation-output/keys.txt> <lexicon.fst> \
  <normalization-profile>
```

See [`PACK_FORMAT.md`](PACK_FORMAT.md) for FST integrity, loading, and immutable
game-lifetime behavior.
