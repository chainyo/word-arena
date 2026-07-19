# French V1 build policy

`word-arena-fr-v1` is built only from the Morphalou 3.1 LMF archive and
checksum in [`sources.toml`](sources.toml). The executable policy is
[`policies/fr-v1.toml`](policies/fr-v1.toml).

## Selected Morphalou data

The builder streams the pinned all-in-one XML member directly from its ZIP and
audits the lemma plus every inflected `orthography` node. It accepts the ten
documented lexical categories: adjectives, adverbs, common nouns,
conjunctions, determiners, interjections, numerals, prepositions, pronouns, and
verbs.

Morphalou states that proper names do not belong in this resource. The builder
still rejects `properName` and `properNoun` defensively. It also rejects:

- entries with the documented `abbreviation` subcategory;
- entries carrying the `locution = oui` marker;
- entries without a grammatical category or with an unknown category;
- forms containing apostrophes, hyphens/dashes, whitespace, digits,
  punctuation, or characters unsupported by the board profile;
- normalized keys outside the 2–15 tile range.

The pinned XML contains 159,250 active lexical entries with 1,135,786 active
lemma/inflection forms. Another 21 entries and 55 forms are deliberately
disabled inside upstream XML comments. The filter report records those inactive
counts separately, while `source_rows`, audit rows, and accepted/rejected totals
refer only to active XML nodes.

## Standard spelling-variant evidence

Morphalou uses `spellingVariantOf` for both accepted standard variants (including
1990 spelling reforms) and explicitly nonstandard forms; it has no general
standard/nonstandard flag. Rejecting every variant would therefore discard valid
French spellings.

French policy V1 instead rejects a spelling variant when it has no lemma-origin
evidence, or when every origin is `lefff`. This reproducible rule excludes the
documentation's nonstandard examples `paske` and `tjs` plus similarly supported
variants, while retaining variants backed by Morphalou2, DELA, Dicollecte,
LGLex, or another source. The evidence rule and excluded origin are versioned in
the policy and reproduced in every audit row for a variant.

## Board normalization

Eligible forms use `fr-basic-latin-fold-v1`. The normalizer uppercases the form,
folds French diacritics (`É` to `E`, `Ç` to `C`, and equivalent decomposed
forms), expands `Œ`/`œ` to `OE` and `Æ`/`æ` to `AE`, then requires only `A`
through `Z`. Length is checked after ligature expansion. Different accented or
ligature forms may intentionally collide on one runtime key; every occurrence
and duplicate decision remains visible in the audit.

The normalized key is also the public board spelling. The game uses only
physical `A` through `Z` tiles: `ÉTÉ` appears as `ETE`, while `ŒUF` occupies the
four tiles `O`, `E`, `U`, `F`. Accents and ligatures remain in source audit data
and are never persisted as special board tiles.

## Deterministic outputs

Run a source build into a directory outside Git:

```bash
cargo run -p word-arena-lexicon-builder -- \
  french-archive /path/to/Morphalou3.1_formatLMF_toutEnUn.zip \
  /tmp/word-arena-fr-build \
  lexicons/policies/fr-v1.toml
```

The builder verifies the archive byte length, SHA-256, exact XML member path,
and uncompressed XML length before producing:

- `keys.txt`: sorted unique normalized keys only;
- `audit.jsonl`: every active source form, original accented text, grammatical
  classification, variant evidence, decision, reason, and duplicate status;
- `filter-report.toml`: active and inactive source totals plus rejection counts;
- `build.toml`: pinned inputs, builder identity, and output checksums.

Generated data stays outside the MIT source repository. A released French pack
is a modified linguistic resource governed by LGPL-LR and must include the
license, notices, modification record, and corresponding legible build/source
materials described in [`README.md`](README.md).
