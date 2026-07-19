# ADR 0002: REST snapshots with WebSocket invalidations

- Status: accepted
- Date: 2026-07-19

## Context

The local web application needs authoritative snapshots and prompt live updates,
while agents need a small transport-neutral application surface. Reconstructing
state independently from a lossy socket stream would create a second source of
truth and make reconnect/privacy behavior harder to prove.

## Decision

V1 uses versioned JSON REST endpoints under `/api/v1` for game creation,
role-bound snapshots, rules, and actions. Every non-creation game endpoint
authenticates an opaque capability from `Authorization: Bearer ...`; request
bodies never select a role or seat.

WebSocket `/events` connections use the same capability header and public
observation scope. They send only `GameInvalidation` values containing schema
version, game ID, and authoritative version. A client connects with
`after_version`, fetches REST when a newer marker arrives, and can reconnect
from its last known version. The server sends the current version immediately
when the client is behind and rejects future cursors.

The server applies a 64 KiB body limit, 128 in-flight HTTP request limit, 64
WebSocket connection limit, 15-second HTTP timeout, 8 KiB WebSocket frame/message
limit, explicit local-development origins, structured HTTP tracing, and
process-signal graceful shutdown.

## Consequences

- REST and the persisted application snapshot remain authoritative.
- Socket loss, duplication, lag, and reconnect do not require event replay in
  the browser and cannot disclose private events.
- Clients make an extra REST read after invalidation; this is acceptable for the
  local-first V1 and simplifies consistency substantially.
- Browser session/capability storage policy remains a WEB-002 responsibility.
- Streamable HTTP MCP will reuse these application/capability boundaries but is
  a separate Phase 3 adapter.

The middleware is `tower-http` 0.7 (MIT), and WebSocket integration tests use
`tokio-tungstenite` 0.29 to match Axum's Tungstenite generation and avoid two
protocol-stack versions in the lockfile.
