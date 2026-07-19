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

All new physical tile IDs must belong to the acting rack and be unique. The
command also carries the acting seat and expected game version, so wrong-seat
and stale actions fail before placement work. New tiles must be in bounds,
target unique empty squares, align, remain contiguous through new or existing
tiles, and connect to the board. The opening move must cover the center. Tile
values are string tokens rather than `char`s, and blanks retain an explicit
assigned token with a zero point value. A regular face cannot be submitted as a
blank or another letter, and a blank cannot be forged from a regular face.

The engine builds the main word and every perpendicular cross word against a
proposed read-only board overlay. Each visible spelling is normalized through
the profile pinned by the ruleset and queried against the same exact lexicon.
Player input is canonicalized before validation and persistence. Every physical
tile, including a blank assignment, becomes exactly one `A` through `Z` board
token. French source spellings remain playable through their normalized board
form: `ÉTÉ` is placed, displayed, scored, emitted, and replayed as `ETE`; `ŒUF`
uses the four physical tiles `O`, `E`, `U`, `F`, never one `Œ` tile. Accented
source forms remain only in lexicon provenance and audit data.

Newly covered letter and word premiums apply independently to every main and
cross word; existing premiums never apply again. Using all seven rack tiles
adds the configured bingo bonus once. Only after ownership, every word, score,
version, refill, public count, and full tile-conservation validation succeeds
does one transaction replace the board, acting rack, bag, scores, scoreless
counter, turn, version, and event streams. Invalid inputs leave an authoritative
snapshot and both event streams byte-for-byte unchanged.

Public move events contain placed tile IDs, score decomposition, draw count,
rack counts, and bag count. A separate seat-private event contains the acting
seat's exact played tiles, exact draws, and resulting rack; it is not included
in another seat's live projection.

Pass and exchange are scoreless atomic turns. Exchange validates unique owned
IDs and the minimum pre-exchange bag size, draws replacements before returning
the selected tiles, and applies the versioned deterministic reshuffle contract.
Resignation ends immediately with the opposing seat as winner and leaves scores
unchanged. Six consecutive scoreless turns end the game and subtract each rack's
value. When a player empties a rack after the bag is exhausted, the opponent's
rack value is subtracted and awarded to the outgoing player; blanks deduct zero.
Scores are signed and checked. Every terminal reason is stored in public state
and in the triggering action event, and all later actions are rejected.

## Persisted identity

The complete `PackIdentity` is recorded in:

- creation state and the creation event;
- every move and finish event;
- public state and the public portion of the authoritative `GameSnapshot`;
- `GameResult`;
- `ReplayBundle`.

`GameSnapshot` is an operator persistence artifact, not a player response: it
contains both racks, exact bag order, and the private seed needed to resume.
Public state contains only the commitment, bag count, and rack counts. A replay
bundle is available only after finish and makes the seed reveal explicit.

Replay verifies the reveal against the creation commitment, then recomputes
every placement, normalized word, premium, refill, public event, and seat-private
transition. Any public or private event-byte difference fails replay. Golden
English and French scenarios cover cross words, blanks, accents, a seven-tile
bingo, depleted-bag refill, and the requirement that `ŒUF` uses the four
physical tiles `O`, `E`, `U`, `F`.
