# Game projections and durable artifacts

The engine owns one authoritative game and derives four non-interchangeable
role types. Transport code must return the narrowest type authorized by its
credential; it must never serialize an authoritative game directly.

## Live role boundaries

- `PublicProjection` contains public state and public events. It includes the
  board, scores, active seat/version, bag count, rack counts, complete ruleset
  identity, exact external lexicon-pack identity, RNG algorithm and seed
  commitment, and completion data. It contains no rack, seed, private draw, or
  future bag order.
- `SeatProjection` adds exactly one named seat's current rack and that seat's
  private transition events. It has no opponent-rack field.
- `HumanSpectatorProjection` adds all current racks and past private events,
  but still has no seed or future bag order. It is a separate concrete type,
  not a privileged variant of `SeatProjection`; application credentials for an
  agent seat must never be accepted by the human-spectator use case.
- `AdministratorProjection` contains the authoritative snapshot and is the only
  role artifact that includes the private seed and ordered future bag.

Every `GameEvent` is created with `EventVisibility::Public`. Every
`PrivateGameEvent` is created with `EventVisibility::SeatPrivate(seat)` and
contains only that seat's removed tiles, draws, and resulting rack. Opening
deals are represented publicly only by counts. These separate event types make
it impossible to append draw data to public history without changing the
schema and failing serialization/replay tests.

The engine defines role data, not authentication. The application service must
bind an opaque seat capability to one fixed `Seat`, keep human-spectator and
administrator authorities in separate credential types, and never place those
credentials in an agent process or workspace.

## Authoritative snapshots

`GameSnapshot` schema V3 contains the ruleset content hash, RNG algorithm,
public state, exact bag order, both racks, private seed, and complete public and
private event histories. It is a persistence artifact, never an API response.
Its `Debug` implementation redacts the bag, racks, and seed.

Resume does not trust decoded state. It validates the static ruleset and exact
external pack, reconstructs a replay bundle from the snapshot's seed and event
histories, replays every transition, and then byte-compares the recomputed
state, bag, racks, and events. Missing, reordered, substituted, or otherwise
inconsistent data is rejected before a `Game` is returned.

## Replay bundles

`ReplayBundle` schema V3 is available only after completion. It contains the
ruleset definition and content hash, exact pack identity by reference, RNG
algorithm and seed reveal, public events, and all deterministic private events.
It never embeds dictionary contents. Replay verifies the seed commitment and
recomputes public and private transitions; exact event mismatches fail.

`PublicReplayBundle` schema V1 is the export-safe counterpart. It includes the
post-game seed reveal and public events but structurally omits private
transitions and racks. The referee reconstructs those internally from the seed
to independently verify the exported public history. Complete `ReplayBundle`
output remains an explicit operator-only export.

Serialization and tamper tests cover both built-in languages, every role,
forbidden projection fields, schema versions, ruleset hashes, pack versions,
seed substitution, physical tile IDs, private draws, event ordering, missing
events, privacy-invalid JSON, and terminal-result changes.
