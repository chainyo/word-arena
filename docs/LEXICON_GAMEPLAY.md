# Lexicon-bound gameplay and replay

English V1 and French V1 rulesets are immutable built-in definitions. Each one
pins the complete allowed lexicon identity: pack ID and version, pack format,
locale, normalization algorithm/version/profile, and content SHA-256. The
engine test suite compares these pins to [`lexicons/registry.toml`](../lexicons/registry.toml)
so an installer release and a gameplay ruleset cannot drift independently.

German and Spanish remain planned languages, but `Ruleset::for_language`
refuses them until each has a separately reviewed offline pack and an explicit
immutable ruleset pin. A decoded or caller-constructed English/French ruleset
is also revalidated against its built-in definition before use; supplying a
different pack and changing the ruleset field together cannot rebind it.

## Runtime boundary

`word-arena-engine::WordValidator` exposes only two operations:

- read the exact immutable `PackIdentity`;
- test an already normalized key for exact membership.

`word-arena-lexicon::LoadedLexicon` implements that boundary. It owns bytes that
already passed complete pack and FST validation; the engine has no installer,
source parser, filesystem lookup, fuzzy matching, or HTTP fallback. The server
loads the exact English and French ruleset identities from the platform data
directory before listening, retains them in `Arc`s, and supplies the same
instances to games.

Game creation, snapshot resume, and replay each require an explicit validator.
An absent validator returns a setup diagnostic. Any difference in the complete
pack identity is rejected before a game is created or restored; another version
is never selected implicitly even if it is installed beside the required one.

## Atomic placement validation

For the current placement vertical slice, all new tiles must be in bounds,
unique, empty, aligned, contiguous through new or existing tiles, and connected
to the board. The opening move must cover the center. Tile values are string
tokens rather than `char`s, and blanks retain an explicit assigned token with a
zero point value.

The engine builds the main word and every perpendicular cross word against a
proposed read-only board overlay. Each visible spelling is normalized through
the profile pinned by the ruleset and queried against the same exact lexicon.
Player input is canonicalized before validation and persistence. Every physical
tile, including a blank assignment, becomes exactly one `A` through `Z` board
token. French source spellings remain playable through their normalized board
form: `ÉTÉ` is placed, displayed, scored, emitted, and replayed as `ETE`; `ŒUF`
uses the four physical tiles `O`, `E`, `U`, `F`, never one `Œ` tile. Accented
source forms remain only in lexicon provenance and audit data.

Only after every word and all checked score/version arithmetic succeeds does a
single transition publish tiles, score, next player, version, and event. An
invalid main or cross word leaves the board, score, turn, version, and event
stream byte-for-byte unchanged.

This slice intentionally uses base language letter values. Board premiums,
racks, bag conservation, exchanges, bingo bonuses, and endgame scoring remain
separate Phase 1 rules work and are not implied by this contract.

## Persisted identity

The complete `PackIdentity` is recorded in:

- creation state and the creation event;
- every move and finish event;
- public state and `GameSnapshot`;
- `GameResult`;
- `ReplayBundle`.

Replay first verifies the bundle ruleset and exact recorded pack, then
recomputes every placement, normalized word, score, state transition, and event.
Any event-byte difference fails replay. Golden English and French tests serialize
and deserialize replay bundles, reconstruct the games, and compare serialized
public state bytes. They also cover three words formed by one placement and an
accent-folded French blank input, canonical board display, and rejection of a
ligature encoded as one physical tile.
