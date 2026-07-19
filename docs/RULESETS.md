# Versioned game rulesets

Word Arena's deterministic engine loads two immutable built-in V1 definitions:

| Ruleset | Physical tiles | Lexicon |
| --- | ---: | --- |
| `english-v1` | 100 | `word-arena-en-world-v1@1.0.0` |
| `french-v1` | 102 | `word-arena-fr-v1@1.0.0` |

The reviewable source fixtures are [`rulesets/english-v1.toml`](../rulesets/english-v1.toml),
[`rulesets/french-v1.toml`](../rulesets/french-v1.toml), and the shared classic
15x15 [`rulesets/classic-board-v1.toml`](../rulesets/classic-board-v1.toml).
They define every tile count/value, rack capacity, bingo bonus, exchange
threshold, scoreless-turn limit, board coordinate, premium, normalization
profile, and exact lexicon identity. Unknown TOML fields fail parsing.

## Physical tokens

One physical tile always occupies one square and carries one canonical `A`
through `Z` token or a blank face. French linguistic spellings are normalized
before board persistence: `ÉTÉ` uses `E`, `T`, `E`, and `ŒUF` uses `O`, `E`,
`U`, `F`. Accents and ligatures remain only in lexicon source/audit metadata.

The domain distinguishes a permanent physical face from a blank's placement
assignment. Every physical tile receives a stable game-local `TileId` so later
bag, rack, board, exchange, replay, and conservation checks can reject forged
or duplicated ownership.

## Validation and identity

The engine expands the compact premium fixture into a complete row-major board,
then rejects incomplete or misordered squares, overlaps, out-of-bounds
coordinates, asymmetric premiums, an invalid center, unsafe limits, missing or
duplicate letter faces, noncanonical tokens, invalid blank values, and checked
arithmetic failures. A built-in ruleset must additionally equal its immutable
compiled definition, so a caller cannot alter both a rule and its identifier.

Each expanded definition receives a SHA-256 identity over a domain-separated
canonical binary encoding. The digest covers schema, ruleset/language IDs,
complete lexicon identity, expanded board/premiums, limits, and ordered tile
distribution. It is independent from JSON/TOML serialization details.

Verify the committed definitions with:

```bash
cargo xtask ruleset verify
```

Current identities:

```text
english-v1 e36324473bd0d7e4203e451d3ae604fbf5323ae654962b22f061df0ca392af58
french-v1  edcd507e82ea304373484880ea7898520654e4d5b73be605476c1ca8c7d9e6ba
```

Any change to a hashed input requires an intentional new ruleset generation;
recorded games and replays never silently adopt revised rules.
