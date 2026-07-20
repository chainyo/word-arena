# Local agent matches

Word Arena's root web route is an agent-first match console. The server owns
agent discovery, game creation, seat credentials, isolated workspaces, harness
processes, and turn advancement; the browser receives only public orchestration
metadata and the appropriate human viewing capability.

## Local workflow

Start the referee and the web application in separate terminals:

```bash
cargo run -p word-arena-server
bun run --cwd web dev
```

Open `http://127.0.0.1:5173`, choose an available CLI for each seat, optionally
replace one seat with a human, and select **Start match**. Agent-versus-agent
opens the trusted human spectator view. Agent-versus-human opens the private
player view for the selected human seat.

The catalog calls each configured executable with `--version`. It does not send
a prompt, invoke a model, validate provider quota, or expose any authentication
material. A compatible CLI can still fail at startup when its native provider
login is missing or expired; that failure appears as content-free match status.

## Trusted local configuration

The server accepts executable overrides only from its process environment:

```text
WORD_ARENA_CODEX_BIN
WORD_ARENA_CLAUDE_BIN
WORD_ARENA_CLINE_BIN
WORD_ARENA_PI_BIN
WORD_ARENA_CODEX_AUTH_FILE
WORD_ARENA_AGENT_SERVER_ORIGIN
```

Codex defaults to the normal `codex` executable and, when present, imports
`~/.codex/auth.json` into each private seat state directory. The source path and
bytes never enter the match request, manifest, public status, or logs. Set
`WORD_ARENA_CODEX_AUTH_FILE` when the login is stored elsewhere. The MCP origin
defaults to `http://127.0.0.1:3000`; override it only when the isolated harness
must reach the referee through a different local origin.

## Lifecycle and authority

`POST /api/v1/matches` creates the authoritative game, issues one short-lived
capability for each seat, and queues the selected harnesses. The runner asks
only the active seat to take a turn, then verifies that the authoritative game
version advanced through MCP. A crash, no-op, unavailable executable, invalid
workspace, or rejected startup ends the match instead of leaving it stalled.

Agents receive their own rack and public board through their seat-scoped MCP
session. They never receive the opponent rack, spectator capability,
administrator snapshot, future bag order, provider credentials belonging to
another seat, or browser-held capabilities. The human spectator may see both
current racks but not future bag order.

## Current milestone boundary

The focused composer, discovery endpoint, isolated turn runner, live status,
and agent-versus-human handoff are implemented. Phase 6A remains open until the
runner also has restart-safe persisted run attribution, cancellation controls,
and credential-free fake-harness scenarios that finish complete English and
French games through MCP. Live provider runs are intentionally opt-in because
they can consume quota.
