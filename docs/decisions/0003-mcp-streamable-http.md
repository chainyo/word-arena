# ADR 0003: authenticated MCP Streamable HTTP

- Status: accepted
- Date: 2026-07-19

## Context

Competitive agents need one vendor-neutral protocol surface over the same
application use cases used by REST. The server is currently a local modular
monolith with capability authentication and Axum. MCP's older HTTP+SSE
transport was replaced by Streamable HTTP, and duplicating protocol framing or
session behavior locally would create an unnecessary compatibility surface.

## Decision

Use the official Apache-2.0 Rust SDK, `rmcp` 2.2.0, through its stateful
Streamable HTTP Tower service. Word Arena advertises and tests the stable MCP
protocol release `2025-11-25`. The endpoint is:

```text
/api/v1/games/{game_id}/mcp
```

Every request must carry a valid seat capability with the `act` scope. The
normal capability verifier checks expiry, revocation, game, role, and scope on
every HTTP request. The gateway then binds each SDK session ID to the game,
seat, and SHA-256 digest of the exact bearer token that initialized it. Another
capability cannot reuse that session, including a capability for the same seat.
Raw tokens are never stored in session state or logs.

The V1 host keeps sessions in process because deployment is a single local
server. The SDK closes idle sessions after five minutes; Word Arena bounds live
bindings at 64 and removes bindings that no longer exist in the SDK manager.
DELETE closes a session. Process shutdown cancels the SDK transport and active
SSE streams.

MCP-001 advertises server metadata with an empty capability object. No game
tools or resources are exposed until their schemas and privacy tests land in
MCP-002 and MCP-003.

The SDK and outer Axum stack jointly enforce:

- JSON/Streamable HTTP framing and protocol-version headers;
- loopback Host and local browser Origin validation;
- 64 KiB bodies, 128 in-flight HTTP requests, and a 15-second HTTP timeout;
- request tracing, JSON-RPC cancellation, session DELETE, and graceful process
  cancellation.

## Consequences

- Agent clients use one standard endpoint without coupling the engine to MCP.
- Session authority is stricter than bearer authentication alone.
- Sessions do not survive a process restart; clients reconnect and initialize a
  new session while authoritative game state remains in SQLite.
- A future multi-process deployment needs a shared SDK session store or sticky
  routing, but no such infrastructure is justified for the local V1.
- The planned stdio bridge remains a client-side adapter to this endpoint, not a
  second game server.

## References

- <https://modelcontextprotocol.io/specification/2025-11-25/basic/transports>
- <https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle>
- <https://github.com/modelcontextprotocol/rust-sdk>
