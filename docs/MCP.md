# MCP access

Word Arena hosts authenticated MCP Streamable HTTP at:

```text
http://127.0.0.1:3000/api/v1/games/{game_id}/mcp
```

Use `Authorization: Bearer <seat-capability>` for initialization and every
later GET, POST, or DELETE. The capability must belong to the path game, have a
competitive seat role, include the `act` scope, and remain active. After
initialization, also send the returned `Mcp-Session-Id` and
`MCP-Protocol-Version: 2025-11-25` headers. A session cannot be transferred to
another token, seat, or game.

The server advertises the competitive tools capability. Every tool uses schema
version `1`, returns structured JSON plus a compact text representation, and is
closed-world: it accesses only the game already bound to the authenticated MCP
session.

| Tool | Purpose | Annotation |
| --- | --- | --- |
| `observe_game` | Read the public board/history plus only the authenticated seat's rack and past private draws. | Read-only, idempotent |
| `get_ruleset` | Read the exact immutable ruleset and pinned offline lexicon identity. | Read-only, idempotent |
| `play_tiles` | Commit owned tile IDs, coordinates, letters, and blank assignments. | Mutating, non-destructive, idempotent |
| `exchange_tiles` | Return selected owned tile IDs and draw replacements. | Mutating, non-destructive, idempotent |
| `pass_turn` | Commit a scoreless pass. | Mutating, non-destructive, idempotent |
| `resign` | Concede and permanently finish the game. | Mutating, destructive, idempotent |

Read tools require `schema_version`. Every mutation additionally requires
`expected_version`, `turn_id`, and `idempotency_key`; V1 requires `turn_id` to
equal `expected_version`. Call `observe_game` immediately before acting. Reuse
an idempotency key only with the byte-equivalent logical command: an identical
retry returns the cached outcome, while different input returns an
`idempotency_conflict` tool error.

The acting game and seat never appear in tool input. They are derived from an
unforgeable request authority inserted only after bearer authentication and
checked again against the MCP session binding. Competitive results never
contain the opponent rack, future bag order, seed, or administrator snapshot.
There is no move preview or best-move tool. Authenticated resources arrive in
MCP-003.

The endpoint accepts the Streamable HTTP methods defined by the protocol:

- `POST` sends JSON-RPC requests and notifications;
- `GET` opens an optional server-to-client SSE stream for an initialized
  session;
- `DELETE` closes the authenticated session.

The main server cancellation token closes active MCP SSE streams during
graceful shutdown. Clients should initialize a fresh session after a server
restart; game state itself remains durable in SQLite.
