# English V1 build policy

`word-arena-en-world-v1` is built only from the SCOWLv1 revision and archive
checksum in [`sources.toml`](sources.toml). The executable policy is
[`policies/en-world-v1.toml`](policies/en-world-v1.toml).

## Selected SCOWL classes

The builder asks SCOWL's own source pipeline to generate its classified `final/`
files. It then imports only the `words` subcategory through size 80 from these
spelling categories:

- `english` and `american`;
- `british` (`-ise`) and `british_z` (`-ize`);
- `variant_1` and `variant_2`, which SCOWL describes as almost-equal and
  generally acceptable variants;
- `british_variant_1` and `british_variant_2`.

Variant level 3 is excluded because SCOWL describes it as seldom used and
potentially incorrect. Canadian, Australian, special, abbreviation,
contraction, upper/name, and proper-name categories are excluded. Raw affix
definition files are never imported; normal inflected words generated and
classified by SCOWL remain eligible.

## Board filter

Eligible source rows must normalize with `en-basic-latin-v1` to 2–15 tokens from
`A` through `Z`. Uppercase source rows, apostrophes, hyphens, punctuation,
digits, spaces, and unsupported characters are rejected before runtime output.

## Deterministic outputs

The builder writes four generated files outside Git:

- `keys.txt`: sorted unique normalized keys only;
- `audit.jsonl`: every source row, its original ISO-8859-1 form, source file,
  SCOWL classification, decision, reason, and duplicate status;
- `filter-report.toml`: totals by source file and rejection reason;
- `build.toml`: pinned inputs, builder identity, and output checksums.

The row invariant is `source_rows = accepted_rows + rejected_rows`. Runtime keys
contain no source metadata; provenance remains in the audit artifact.

SCOWL's V1 source scripts assume GNU `grep`, GNU `find -printf`, and sequential
Make execution. The Rust preparation adapter replaces only the extracted
symlink-dependency helper with a POSIX equivalent, pre-generates those
dependencies, uses GNU `grep` (`grep` on Linux or `ggrep` on macOS), and invokes
upstream Make sequentially. This adaptation changes no source data or category
logic.
