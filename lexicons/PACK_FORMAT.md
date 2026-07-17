# Lexicon pack format V1

This contract defines immutable Word Arena lexicon packs. Pack payloads are
separately licensed data artifacts; the parser and validator are MIT-licensed
application code. Format V1 supports the initial English and French packs.

## Directory contract

Every unpacked pack has this minimum shape:

```text
manifest.toml
lexicon.fst
curation/additions.toml
curation/removals.toml
LICENSE
SOURCE.md
THIRD_PARTY_NOTICES
```

Optional audit or build artifacts are allowed only when every regular file is
listed in `manifest.toml`. Symlinks, non-UTF-8 paths, absolute paths, `.` or `..`
components, backslash separators, duplicate records, missing required records,
and unlisted files are rejected. Paths in the manifest are UTF-8 and always use
`/`, independent of the host platform.

## Strict manifest schema

The canonical schema is represented by the public Rust types in
`word-arena-lexicon`. Unknown fields are rejected at every table. A manifest has
these required fields:

```toml
format_version = 1
pack_id = "word-arena-en-world-v1"
pack_version = "1.0.0"
locale = "en"
word_count = 0
content_sha256 = "<64 lowercase hexadecimal characters>"

[normalization]
algorithm = "word-arena-board-key"
version = 1
profile = "en-basic-latin-v1"

[source]
id = "scowl-v1-2024-07-22"
revision = "b22230cc5250887737fdefe9ca4c9d9d01230eaa"
archive_sha256 = "65e4891913a252659efd9a464b923124940082b5bd4da4878d1e7fbf1b80bc50"
license_id = "LicenseRef-SCOWL-v1"

[policy]
id = "en-world-filter"
version = 1

[builder]
name = "word-arena-lexicon-builder"
version = "0.1.0"

[[files]]
path = "lexicon.fst"
size_bytes = 123
sha256 = "<64 lowercase hexadecimal characters>"
```

`pack_version` and `builder.version` use Semantic Versioning. Source, policy,
builder, pack format, and normalization versions remain separate so changing
one input cannot silently redefine another contract.

## Content checksum

Every listed file carries its exact byte length and SHA-256. The pack-level
`content_sha256` is also SHA-256 over this byte stream:

1. the domain bytes `word-arena-pack-content-v1\0`;
2. file records sorted by the unsigned UTF-8 bytes of their manifest path;
3. for each record, the path byte length as unsigned 64-bit big endian;
4. the exact path bytes;
5. the payload byte length as unsigned 64-bit big endian;
6. the exact payload bytes.

The manifest does not checksum itself, avoiding a circular digest. Its immutable
consumer reference combines `pack_id`, `pack_version`, `format_version`,
`locale`, the complete normalization identity, and `content_sha256`. Raw bytes
are never newline-normalized, and directory enumeration order is irrelevant, so
the same manifest and files produce the same identity on supported platforms.

## Exact-membership keys

`lexicon.fst` is a set encoded by the deterministic `fst` 0.4 format. It stores
unique normalized keys in unsigned UTF-8 byte order and carries no values. This
compact representation is directly memory-mappable; the safe V1 reader instead
owns one verified byte buffer so an in-use game remains independent of later
filesystem changes without introducing an unsafe mapping boundary. Membership
is an exact borrowed-byte comparison and allocates nothing per lookup. Runtime
lookup never performs fuzzy matching or uses a live HTTP dictionary service.
Whitespace and control characters are invalid keys.

Build an index from the sorted output of the curation stage with:

```bash
cargo run -p word-arena-lexicon-builder -- \
  index-compile <curated-keys.txt> <lexicon.fst> <normalization-profile>
```

Compilation streams its input, rejects invalid, duplicate, or out-of-order
keys, and atomically publishes without overwriting. Two builds from identical
keys and profile produce byte-identical FSTs. The manifest records the exact
index byte length, SHA-256, and fully enumerated key count.

The runtime loader validates the complete pack before returning a queryable
lexicon, rereads and rechecks the retained FST bytes against their manifest
descriptor, verifies the embedded FST checksum, enumerates every key through
the pinned normalization profile, and compares the observed count with
`word_count`. Corrupt, truncated, non-set, unsupported, non-normalized, or
mismatched indexes are rejected. Once loaded, the manifest, identity, and
index bytes are owned by that instance and never hot-swapped.

Normalization algorithm `word-arena-board-key` version 1 has two profiles:

- `en-basic-latin-v1`: apply Unicode uppercasing, then require every output
  scalar to be `A` through `Z`.
- `fr-basic-latin-fold-v1`: expand `Œ/œ` to `OE` and `Æ/æ` to `AE`; apply
  Unicode uppercasing and canonical decomposition; remove combining marks; then
  require every output scalar to be `A` through `Z`.

The original French source form remains builder audit data; only its normalized
board key enters exact membership. A future normalization change requires a new
normalization version or profile and therefore a different immutable identity.

## Compatibility rules

| Consumer | Required behavior |
| --- | --- |
| Ruleset starting a game | Match the complete pinned pack identity exactly. |
| Replay | Load the exact identity recorded by the game; never substitute a newer pack. |
| Active game | Keep the starting identity for the game lifetime; never hot-swap cached content. |
| Cache install | Treat an exact identity as idempotent and install new versions side by side. |
| Same pack ID and version, different identity | Reject as a conflicting immutable release. |

Validation errors identify the unsupported version, invalid field, unsafe or
missing path, or expected and calculated checksum so setup tooling can present a
direct recovery action.
