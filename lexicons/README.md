# Lexicon sources and licensing

Word Arena application code is MIT-licensed. Lexicon source data and generated
packs are separate works governed by their own licenses. The canonical,
machine-readable source pins are in [`sources.toml`](sources.toml); the exact
upstream license texts are stored in [`licenses/`](licenses/).

The separately versioned pack manifest, integrity, normalization, and
compatibility contract is documented in [`PACK_FORMAT.md`](PACK_FORMAT.md) and
implemented by `crates/lexicon`.

Reviewed additions, removals, two-person approvals, and their reproducible
changelog contract are documented in [`CURATION.md`](CURATION.md). The typed V1
inputs live under [`curation/`](curation/).

No upstream word archive or generated word list is committed to Git. The
workspace `cargo xtask` commands download pinned archives, reject checksum
mismatches, build deterministic separately licensed artifacts, and install the
released identities listed in [`registry.toml`](registry.toml). Local setup,
data locations, pack lifecycle commands, recovery, and source rebuilds are
documented in [`docs/LOCAL_SETUP.md`](../docs/LOCAL_SETUP.md). NWL, Collins,
ODS, and results from online dictionary checkers are not inputs to either V1
pack.

Release packs are independently versioned immutable GitHub data releases. See
[`RELEASING.md`](RELEASING.md) for the two-build byte comparison, compliant
asset set, draft-first publication, checksum verification, and deletion
protection contract.

## English source

`word-arena-en-world-v1` derives from SCOWLv1's `v1` branch at commit
`b22230cc5250887737fdefe9ca4c9d9d01230eaa`, committed on 2024-07-22. The
selected source archive has SHA-256
`65e4891913a252659efd9a464b923124940082b5bd4da4878d1e7fbf1b80bc50`.

SCOWLv1 permits use, modification, redistribution, and sale when its copyright
and permission notices are retained. Its size-80 material incorporates sources
with additional notice conditions, including WordNet, UKACD, VarCon, and
Ispell. The complete upstream copyright file is retained verbatim as
[`SCOWL-v1-Copyright.txt`](licenses/SCOWL-v1-Copyright.txt) and must ship with
the English pack. In particular, the UKACD notice must be displayed prominently
and included verbatim, and modified Ispell-derived material must be marked.

## French source

`word-arena-fr-v1` derives from the canonical Morphalou 3.1 LMF all-in-one
archive published by ATILF through ORTOLANG. The selected archive has SHA-256
`f49903f11eb73e3a8e42249415e9300cac1ea812b5d93443d6ef4aa53135ee59`.

Morphalou is licensed under the Lesser General Public License for Linguistic
Resources (LGPL-LR). The compiled French pack is a modified linguistic
resource: it must remain under LGPL-LR at no charge, identify modifications and
their dates, include the complete license and notices, and provide the
corresponding machine-readable legible form and build materials beside the
compiled artifact. The MIT application remains a separate work that reads a
replaceable resource through a stable interface.

Use this citation for Morphalou:

> ATILF (2023). *Morphalou*, version 3.1. ORTOLANG (Open Resources and
> Tools for Language). <https://hdl.handle.net/11403/morphalou/v3.1>

## German and Spanish candidates

The approved, immutable source and license choices for the future German and
Spanish packs are documented in
[`MULTILINGUAL_SOURCE_REVIEW.md`](MULTILINGUAL_SOURCE_REVIEW.md). Their linguistic
imports remain gated by the committed native-speaker review requirements; no
German or Spanish word data is currently imported or released.

## Release compliance rule

A lexicon pack must not be released until an automated check confirms that its
source ID, archive checksum, license file, notices, modification record, and
reproducible source/build materials match this registry. This repository records
the compliance approach; it is not a substitute for professional legal advice.
