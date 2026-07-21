# Web application foundation

The React application is a local game workspace, not a marketing site. `/`
opens a compact lineup builder beside a tabbed **Live** and **Finished** match
archive. Two player rows default to installed, compatible agents; dashed rows
add player three or four without a separate player-count control. Each active
row owns its agent selector, optional model override, and human toggle, and at
most one seat may be a local human. The composer discovers local CLIs without
invoking a model and opens the spectator view as soon as the referee accepts
the match. Its routes have explicit authority requirements:

- `/games/{game_id}/public`
- `/games/{game_id}/player` (competitive seat; `/seat` remains compatible)
- `/games/{game_id}/spectator` (trusted human spectator)
- `/games/{game_id}/replay` (trusted human spectator, finished games only)
- `/tournaments` (local operator lobby)
- `/tournaments/{tournament_id}/standings` (public aggregates)
- `/agents/{agent_id}` (public aggregates)

The route selects a projection shape, never an authorization role. The Rust
server derives authority exclusively from the supplied capability and rejects a
token issued for another role or game.

## Credential and cache policy

Opaque capabilities live only in the JavaScript process-memory vault. They are
not written to the route, URL query, `localStorage`, `sessionStorage`, IndexedDB,
service workers, or TanStack Query keys. A reload or tab close clears them.
For a game in the server's local agent-match index, the spectator and replay
routes recover by requesting a fresh short-lived human-spectator capability
from the trusted local operator endpoint. Manually opened external games still
require their capability again. Query keys contain only the server origin, game
ID, and projection kind, and cached values are already decoded role-appropriate
snapshots.

Each response passes a strict runtime decoder in addition to TypeScript static
checking. Public decoders fail closed if rack/private/bag/seed/snapshot fields
appear. Seat decoders reject opponent-rack, bag, seed, and administrator data.
Spectator decoders accept all current racks but still reject the future bag,
seed, and administrator snapshot.

Agent-match creation returns public and human-spectator capabilities once, plus
one browser seat capability only when the operator explicitly selected a human.
The operator workspace places each into its separate memory-vault slot. Agent
seat capabilities go directly from the trusted server orchestrator into that
seat's isolated process and are never returned to the browser.

The local match list contains only versioned public orchestration metadata:
game ID, language, mode, phase, scores, timestamps, seat identities, and safe
runner state. It is persisted with SQLx in SQLite. It never contains a raw
capability, rack, bag order, seed, prompt, or transcript.

## Authoritative updates

The REST snapshot is the sole game state. A WebSocket carries only the V1
`schema_version`, `game_id`, and `version` invalidation marker. A newer marker
invalidates the exact projection cache and causes a fresh REST request. Dropped
connections retry with bounded backoff and reconnect using the last decoded
snapshot version.

When the browser reports offline state, reconnect timers stop and resume
immediately on the next online event without creating duplicate sockets. A
dropped stream or failed refresh leaves the last decoded board visibly marked
as stale and disables seat actions. A manual retry always invalidates the cache
before fetching. HTTP 401 is presented as an expired/revoked capability with a
memory-vault reset; an action `version_conflict` refreshes the authoritative
snapshot, clears the obsolete draft, and restores focus to game status. Other
HTTP 409 errors retain their specific referee message.

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

## Accessibility, themes, and responsive behavior

Every workspace header provides a visible-on-focus skip link to a focusable
main landmark. Player/configuration, board, history, replay, and export regions
have explicit labels. The board has a concise committed/staged/latest-move
narration, while individual squares retain exact coordinate and premium names.
Move rows have complete summaries. One polite, atomic live region announces
connection and newest-move changes; errors remain assertive without making the
whole board live. After a committed action or conflict refresh, focus returns
to the board status heading rather than disappearing during the versioned
render.

Light, dark, and system modes use only semantic shadcn/game tokens. The selected
theme may persist locally (capabilities never do), tracks system changes, sets
the browser color scheme, and avoids transition flashes. Tile shadows are also
theme tokens rather than component color literals.

At narrow widths and high zoom the 15-by-15 board becomes a named,
keyboard-focusable horizontal scroll region with a 42 rem minimum grid, keeping
every square at least 44 CSS pixels. Coarse-pointer controls and rack tiles have
44-pixel minimum targets, page grids stack at small breakpoints, and long replay
identities wrap. Reduced-motion mode removes smooth scrolling/long animations
and disables replay auto-play while keeping previous/next stepping available.
Biome accessibility rules plus Bun semantic-render tests verify landmarks,
narration, touch/overflow CSS, theme resolution, connection messaging, and
motion alternatives. Axe plus desktop/mobile Playwright scenarios verify the
real operator, player, spectator, reconnect, authentication, privacy, terminal,
and replay flows against a deterministic V1 fixture referee.

## Replay and aggregate views

The replay route loads a persisted artifact only after game completion and only
with a human-spectator capability. Controls rebuild board, score, rack counts,
bag count, turn, and phase from recorded events without mutating the live game.
The authorized view shows the seed reveal, RNG, exact ruleset digest, exact
lexicon pack version/digest, and the count of private transitions.

Public replay export is a separate typed projection. It includes the exact
ruleset content, lexicon identity, public event stream, RNG identity, and
post-game seed reveal needed for reproducibility, but omits private transitions,
racks, capabilities, live snapshots, and transcripts. The share action copies
only a public route and never a token. Event filters use bounded pagination; no
virtualization is justified for one game's measured event volume.

Tournament lobby, standings, and agent-detail routes have filters, pagination,
and explicit empty states. They intentionally show no fabricated records until
the authoritative tournament/statistics repositories in Phase 6 populate them.

## Contract verification

[`contracts/web-api-v1.json`](../contracts/web-api-v1.json) pins the API schema,
projection/replay schemas, paths, browser protocol, and invalidation fields.
Rust tests compare it to the authoritative server constants, while Bun tests
compare the typed client and exercise decoding, route authority, replay
stepping, statistics formatting, export privacy, authentication, cache keys,
and reconnect decisions.

```bash
scripts/web/verify-contract.sh
bun run --cwd web check
bun run --cwd web test:e2e
bun run --cwd web check:full
```
