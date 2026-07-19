# Gated Hunspell build foundation

German and Spanish source imports are not enabled yet. Their committed review
records remain `pending`, so neither language has a registry pack, release
artifact, setup download, ruleset, or production word list.

The repository does contain the source-independent portion of their future
build path:

- `HunspellPolicy` strictly binds locale, source revision/archive pin, exact
  `.aff`/`.dic` members, board normalization, length limits, and review record.
- `ApprovedNativeReview::load` is the only constructor for an archive-import
  approval. It rejects pending, incomplete, non-HTTPS, mismatched, or malformed
  reviewer evidence before inspecting source bytes.
- `build_hunspell_from_archive` verifies the archive size and SHA-256, reads
  only the two exact configured regular-file members, and refuses missing or
  duplicate members.
- A pinned pure-Rust `zspell` parser expands stems and affix rules. Normal and
  no-suggest accepted forms are considered; Hunspell-forbidden forms are
  excluded. Output is a sorted key file, deterministic JSONL audit, filter
  report, and checksummed build metadata compatible with the existing pack
  assembly pipeline.
- Hand-authored synthetic fixtures prove inflection expansion, forbidden and
  no-suggest behavior, policy boundaries, normalized-key collisions, corrupt
  pins, approval gating, and clean-build byte equivalence without copying or
  importing an upstream word list.

The provisional board-key profiles follow the already committed A-Z policies:
German decomposes diacritics and Unicode uppercasing expands `ß` to `SS`;
Spanish decomposes diacritics, including the tilde on `ñ`. The original source
form remains in the audit while only the canonical key enters the FST. These
choices—and whether the spellchecking sources provide sufficient proper-name,
abbreviation, regional, and productive-compound boundaries—must be accepted or
changed by the required native-language reviews before real data is imported.

After those reviews, OPS-002 still needs curation bundles, compliant GPL pack
assembly/notices and corresponding source materials, immutable registry and
release pins, German/Spanish physical rulesets, installer/release integration,
and full games/replay verification. Until then its task and project-plan boxes
remain intentionally unchecked.
