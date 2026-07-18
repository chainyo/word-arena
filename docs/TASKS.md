# Word Arena implementation tasks

This is the implementation-ready backlog for the next vertical slice. Keep task
status aligned with [`PROJECT_PLAN.md`](PROJECT_PLAN.md), and check an item only
after its verification criteria pass. The current priority is a deterministic,
offline English and French game using separately licensed lexicon packs.

## V1 outcome

- A first-time contributor runs `cargo xtask setup`.
- The command installs frontend dependencies and downloads any missing pinned
  English and French lexicon pack artifacts.
- Downloads are checksum-verified and installed atomically with their source and
  license metadata.
- The server can then create, play, and replay English and French games without
  contacting a dictionary website.
- Every game records the immutable lexicon pack ID and content checksum.
- The packs are named Word Arena lexicons and make no claim of exact NWL,
  Collins, or ODS compatibility.

## Pack contents

Each released pack must contain at least:

```text
manifest.toml
lexicon.fst
curation/additions.toml
curation/removals.toml
LICENSE
SOURCE.md
THIRD_PARTY_NOTICES
```

The manifest must identify the pack ID and version, locale, board normalization,
source revision and archive checksum, source license, filter-policy version,
builder version, word count, and compiled-content checksum.

## Tasks

### LEX-001: Pin sources and record license obligations

- [x] Resolve and record the full SCOWLv1 commit corresponding to the selected
  stable 2024 revision; capture its archive SHA-256 and complete required
  copyright notices.
- [x] Pin Morphalou 3.1 from its canonical ATILF/ORTOLANG release; capture its
  archive SHA-256, citation, LGPL-LR text, and modification/distribution
  obligations.
- [x] Add a machine-readable source registry without vendoring either upstream
  archive or any proprietary word list into Git.
- [x] Document that SCOWLv1 and Morphalou packs are data artifacts with licenses
  separate from the MIT application.

Verification:

- Every source URL, revision, checksum, license, attribution, and redistribution
  obligation is reviewable without running the builder.
- A clean license review finds no dependency on NWL, Collins, ODS, scraped
  checker results, or another source without explicit redistribution rights.

### LEX-002: Define the versioned lexicon pack contract

- [x] Define and validate the pack manifest schema.
- [x] Define normalized exact-membership keys as UTF-8 strings and version the
  normalization algorithm independently from the source and builder.
- [x] Define pack compatibility rules for rulesets, replays, cache updates, and
  active games.
- [x] Reject missing files, unknown required fields, unsupported format versions,
  and checksum mismatches with actionable errors.

Verification:

- Golden manifests cover both V1 languages plus malformed and incompatible
  packs.
- The same manifest and files calculate the same content identity on supported
  platforms.

### LEX-003: Build `word-arena-en-world-v1`

- [x] Import only the selected SCOWLv1 normal-word categories through the
  size-80 boundary for the agreed American/British profile.
- [x] Exclude proper names, uppercase/name lists, abbreviations, contractions,
  affixes, special lists, punctuation, digits, spaces, and hyphenated entries.
- [x] Restrict playable keys to the configured board alphabet and length range.
- [x] Preserve source classification and original form in build audit output,
  without placing unnecessary source metadata in the runtime index.

Verification:

- Two clean builds from the pinned archive are byte-identical.
- Filter reports account for every accepted and rejected source row.
- Hand-authored tests cover dialect variants, inflections, names, contractions,
  punctuation, unsupported characters, and length boundaries.

### LEX-004: Build `word-arena-fr-v1`

- [x] Import standard single-token Morphalou 3.1 lemmas and inflected forms.
- [x] Exclude proper names, abbreviations, locutions, explicitly nonstandard
  spellings, punctuation, digits, spaces, hyphens, and unsupported tokens.
- [x] Preserve the original accented source form for audit while producing the
  deterministic board key defined by the French ruleset.
- [x] Specify and test accent and ligature behavior, including `É`, `Ç`, `Œ`,
  and normalization collisions.

Verification:

- Two clean builds from the pinned archive are byte-identical.
- Filter reports account for every accepted and rejected source row.
- Hand-authored tests cover common inflections, accents, ligatures, locutions,
  names, abbreviations, nonstandard tags, and length boundaries.

### LEX-005: Add transparent curation governance

- [x] Define typed additions and removals files for each pack.
- [x] Require every override to contain a normalized word, action, reason,
  openly usable supporting source, author, reviewer, and date.
- [x] Require two-person review for changes to two-letter words, normalization
  rules, or broad filters because they have disproportionate game impact.
- [x] Generate a release changelog showing added and removed playable keys.
- [x] Add an issue template for disputed words without accepting copied
  proprietary lists as evidence.

Verification:

- Invalid, duplicate, conflicting, or undocumented overrides fail the build.
- Every released word-set change is attributable and reproducible.

### LEX-006: Compile and load the offline runtime index

- [x] Compile sorted normalized keys into a compact deterministic FST or
  equivalent memory-mappable exact-membership index.
- [x] Add a dependency-light runtime reader with `contains`, manifest access,
  and integrity verification; keep source parsing out of the game engine.
- [x] Verify a pack before making it available to a new game.
- [x] Keep an in-use pack immutable, even if a newer version is installed.
- [x] Do not implement a live HTTP dictionary fallback.

Verification:

- Lookup results are deterministic and allocation-conscious under repeated use.
- Corrupt, truncated, mismatched, and unsupported packs are rejected.
- Tests run with network access unavailable.

### LEX-007: Download packs during first-time local setup

- [x] Add a workspace `xtask` crate and the canonical `cargo xtask setup`
  command.
- [x] Have setup verify required tools, run `bun install --cwd web`, then install
  the pinned English and French packs when they are not already present.
- [x] Read artifact URLs and SHA-256 values from a committed pack registry.
- [x] Download into a staging location, verify the complete archive, verify its
  internal manifest and license files, and atomically move it into the platform
  data directory.
- [x] Use an OS-appropriate data/cache directory with a documented
  `WORD_ARENA_DATA_DIR` override; do not commit downloaded archives or compiled
  packs.
- [x] Make setup idempotent and safe under interruption or concurrent execution.
- [x] Add `cargo xtask lexicon list`, `verify`, `install`, and `remove` commands,
  plus `setup --offline` for validating an existing installation without any
  network request.
- [x] Provide `cargo xtask lexicon build --from-source` to download the pinned
  upstream archives and reproduce the release artifacts and checksums.

Verification:

- A clean-machine integration test downloads both fixtures from a local test
  server and installs them successfully.
- A second setup performs no pack downloads.
- Bad checksums, missing notices, interrupted downloads, and unavailable
  networks leave the previous installation untouched and return clear recovery
  instructions.
- After one successful setup, the server starts and validates both languages
  while the network is unavailable.

### LEX-008: Integrate packs with rules, games, and replay

- [x] Bind every language ruleset to an allowed lexicon pack ID, format version,
  normalization version, and content checksum.
- [x] Inject the loaded exact-membership boundary into deterministic move
  validation.
- [x] Validate every main and cross word before scoring or committing a move.
- [x] Record the pack identity in game creation, events, snapshots, results, and
  replay bundles.
- [x] Refuse to create or resume a game if its exact pack is unavailable rather
  than silently selecting another version.

Verification:

- Golden English and French games replay with byte-equivalent public state.
- Missing or substituted packs fail before state mutation.
- Tests cover multiple words formed by one placement and blank-tile
  normalization.

### LEX-009: Publish compliant, reproducible pack releases

- [x] Build English and French artifacts in CI from pinned upstream archives.
- [x] Rebuild each pack twice and compare bytes before publishing.
- [x] Publish compiled packs, committed checksums, license/notices, curation
  files, build instructions, and the corresponding legible/source form required
  by each license.
- [x] Keep pack releases versioned independently from application releases.
- [x] Prevent deleting a pack version referenced by a published replay without
  an archival replacement.

Verification:

- A release artifact verifies against the committed registry and can be rebuilt
  to the same checksum.
- License and attribution files survive installation and are visible through a
  CLI command and operator UI/API metadata.

### LEX-010: Finish documentation and quality gates

- [x] Update the README quick start to begin with `cargo xtask setup` and explain
  the first-install download and offline runtime behavior.
- [x] Document pack provenance, curation policy, dispute process, data location,
  update/rollback commands, and removal behavior.
- [x] Add CI checks for source pins, checksums, manifests, overrides, licenses,
  deterministic builds, offline lookup, and replay compatibility.
- [x] State prominently that the default packs are Word Arena lexicons and are
  not official SCRABBLE tournament dictionaries.

Verification:

- A clean contributor setup follows only documented commands.
- All repository checks pass with no downloaded lexicon data accidentally staged
  in Git.

## Later tasks

- [ ] Evaluate explicitly redistributable German and Spanish source lexicons.
- [ ] Support separately licensed operator-supplied NWL, Collins, ODS, or other
  tournament packs without exposing or redistributing their contents.
- [ ] Add signed registry metadata if remote pack distribution grows beyond the
  initial project-controlled release channel.
