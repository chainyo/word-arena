# Web application foundation

The React application is a local game workspace, not a marketing site. `/`
opens a board-first connection screen. An authenticated workspace uses one of:

- `/games/{game_id}/public`
- `/games/{game_id}/seat`
- `/games/{game_id}/spectator`

The route selects a projection shape, never an authorization role. The Rust
server derives authority exclusively from the supplied capability and rejects a
token issued for another role or game.

## Credential and cache policy

Opaque capabilities live only in the JavaScript process-memory vault. They are
not written to the route, URL query, `localStorage`, `sessionStorage`, IndexedDB,
service workers, or TanStack Query keys. A reload or tab close deliberately
clears them. Query keys contain only the server origin, game ID, and projection
kind, and cached values are already decoded role-appropriate snapshots.

Each response passes a strict runtime decoder in addition to TypeScript static
checking. Public decoders fail closed if rack/private/bag/seed/snapshot fields
appear. Seat decoders reject opponent-rack, bag, seed, and administrator data.
Spectator decoders accept both current racks but still reject the future bag,
seed, and administrator snapshot.

## Authoritative updates

The REST snapshot is the sole game state. A WebSocket carries only the V1
`schema_version`, `game_id`, and `version` invalidation marker. A newer marker
invalidates the exact projection cache and causes a fresh REST request. Dropped
connections retry with bounded backoff and reconnect using the last decoded
snapshot version.

## Player interaction boundary

Private seat routes render the exact rack returned by the referee. Selecting a
rack tile and board square creates a visually distinct local draft; it does not
remove a rack tile, calculate a score, advance a turn, or predict a draw. Blank
assignment accepts only the physical A–Z board alphabet. The pinned French
normalizer still accepts accented dictionary spellings while the committed board
remains accent-free.

Play, exchange, pass, and resign controls require confirmation and submit the
current authoritative version, turn number, and a fresh idempotency key. Pending
controls lock until a response arrives. Only the returned seat projection may
replace the board, rack, scores, deadline, and history; a rejection leaves the
draft visible with the referee's safe error.

The board uses semantic ordered-list coordinates rather than canvas. One square
is in the tab order and arrow keys move focus across all 225 squares, so a rack
tile can be selected and placed without a pointer. Premium labels, tile values,
staged state, and current racks have explicit accessible names. English and
French display values come from the immutable rules response with a checked-in
V1 display fallback for spectator capabilities that do not include public-rules
scope.

Browsers cannot set the `Authorization` header on WebSocket handshakes. They
therefore send `word-arena-v1` plus the opaque capability as requested
subprotocols; the server authenticates the latter and selects only the safe
`word-arena-v1` protocol in its response. Capabilities are never placed in the
WebSocket URL.

## Contract verification

[`contracts/web-api-v1.json`](../contracts/web-api-v1.json) pins the API schema,
projection schema, paths, browser protocol, and invalidation fields. Rust tests
compare it to the authoritative server constants, while Bun tests compare the
typed client and exercise decoding, privacy, authentication, cache keys, and
reconnect decisions.

```bash
scripts/web/verify-contract.sh
bun run --cwd web check
```
