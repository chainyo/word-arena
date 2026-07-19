# German and Spanish source selection

Status: source licenses approved; language imports gated pending the native-speaker
reviews recorded in `reviews/de-v1.toml` and `reviews/es-v1.toml`.

This review selects immutable UTF-8 Hunspell snapshots from
`wooorm/dictionaries` at commit
`8cfea406b505e4d7df52d5a19bce525df98c54ab`. That repository identifies the
German data as generated from igerman98 and the Spanish data as generated from
RLA-ES. Word Arena consumes only the pinned `index.dic` and `index.aff` inputs;
the normalized repository snapshot provides an immutable archive when the
German upstream site does not publish a content-addressed release.

## Legal decision

The German notice explicitly permits redistribution and modification under
GPL version 2 or 3. Word Arena elects `GPL-3.0-only`. The Spanish notice
explicitly offers a disjunctive choice of GPL 3 or later, LGPL 3 or later, or
MPL 1.1 or later. Word Arena elects `GPL-3.0-or-later`. These choices avoid
combining incompatible license alternatives and permit one release procedure
for both generated packs.

Generated German and Spanish packs are separate GPL resources. Each release
must carry the exact committed source notice, complete GPL terms, modification
notice, corresponding source archive, deterministic importer/build materials,
and checksum metadata. The MIT application remains independently replaceable
and does not incorporate either word list. This repository does not claim that
the packs are official German or Spanish tournament dictionaries.

The archive URL, byte length, SHA-256, exact source paths, revision URL,
notice URL, notice SHA-256, attribution, selected SPDX expression, and concrete
redistribution obligations are machine-readable in `sources.toml`. A release
must fail if any of these pins or committed notices drift.

Primary records reviewed:

- German immutable package and notice:
  <https://github.com/wooorm/dictionaries/tree/8cfea406b505e4d7df52d5a19bce525df98c54ab/dictionaries/de>
- German upstream provenance: <https://www.j3e.de/ispell/igerman98/index_en.html>
- Spanish immutable package and notice:
  <https://github.com/wooorm/dictionaries/tree/8cfea406b505e4d7df52d5a19bce525df98c54ab/dictionaries/es>
- Spanish upstream project: <https://github.com/sbosio/rla-es>
- Selected GPL terms: <https://www.gnu.org/licenses/gpl-3.0.html>

## Linguistic gate

Source licensing approval is not linguistic approval. Before an importer may
produce either pack, a native or professionally fluent reviewer must approve
the matching policy and replace its pending review record with a dated decision,
rationale, stable HTTPS evidence URL, and reviewer identity. Review covers:

- which common-word and inflected forms are admitted;
- proper-name, abbreviation, punctuation, multi-token, and board-length filters;
- noun/adjective/verb inflection, gender, and plural behavior;
- accent folding and source-spelling audit retention;
- German `ß` and Spanish digraph/token boundaries;
- every physical tile token and representative golden fixtures.

Until that evidence exists, repository audit permits the recorded `pending`
state but no German or Spanish importer, registry pack, setup download, or
release entry may be added. Approval must be checked mechanically before OPS-002
can import data.
