# CLI and MCP stdio bridge

`word-arena` is a thin client for the authoritative server. It does not contain
game rules, score moves, generate placements, or expand a credential beyond its
server-defined game and seat.

Build or run it from the workspace:

```bash
cargo build -p word-arena-cli
cargo run -p word-arena-cli -- --help
```

## Configuration

Each value uses the same precedence: command-line flag, environment variable,
local TOML file, then a default where one exists.

| Value | Flag | Environment | Default |
| --- | --- | --- | --- |
| Server origin | `--server` | `WORD_ARENA_SERVER` | `http://127.0.0.1:3000` |
| Game ID | `--game-id` | `WORD_ARENA_GAME_ID` | none |
| Capability | `--token` | `WORD_ARENA_TOKEN` | none |
| Timeout | `--timeout-ms` | `WORD_ARENA_TIMEOUT_MS` | `15000` |
| Config path | `--config` | `WORD_ARENA_CONFIG` | `$XDG_CONFIG_HOME/word-arena/config.toml`, then `$HOME/.config/word-arena/config.toml` |

The config file is strict TOML:

```toml
server_url = "http://127.0.0.1:3000"
game_id = "game-example"
token = "wa_cap_v1..."
timeout_ms = 15000
```

On Unix, the CLI refuses a config accessible by group or other users:

```bash
chmod 600 "$XDG_CONFIG_HOME/word-arena/config.toml"
```

Prefer the environment or private config over `--token`, because command-line
arguments may be visible to other local processes. Token-bearing configuration
has redacted debug output, remote errors never echo response bodies, and the
CLI never writes a capability to stdout or stderr.

## Commands

```bash
word-arena health
word-arena auth
word-arena observe
word-arena action \
  --expected-version 0 \
  --turn-id 0 \
  --idempotency-key pass-0 \
  --action-json '{"type":"pass"}'
word-arena replay export --output game-example-seat-one.json
word-arena mcp stdio
```

`health` needs only the server origin. `auth`, `observe`, and `replay export`
use the REST seat projection and therefore require `observe_seat`. `action`
requires `act`. A capability may contain both scopes. The replay export is
deliberately a seat replay: it contains public history and only that credential's
private transitions, never the opponent rack, future bag, seed, or administrator
snapshot. Export files are created without overwrite and with mode `0600` on
Unix.

Normal command results are versioned JSON on stdout. Diagnostics are confined
to stderr. Exit codes are deterministic: `2` usage/configuration, `3`
authentication, `4` remote service, `5` protocol, `6` I/O, and `130`
interruption.

## MCP stdio

`word-arena mcp stdio` lets a standard local MCP client use the authenticated
Streamable HTTP game endpoint without implementing HTTP. Set a seat capability
with `act`, plus its game and server:

```bash
WORD_ARENA_SERVER=http://127.0.0.1:3000 \
WORD_ARENA_GAME_ID=game-example \
WORD_ARENA_TOKEN='wa_cap_v1...' \
word-arena mcp stdio
```

The bridge accepts one compact JSON-RPC message per stdin line and writes only
JSON-RPC messages to stdout. It preserves the server-issued MCP session ID,
forwards POST responses, opens the optional SSE notification stream, reconnects
that stream with bounded exponential backoff, sends session DELETE on clean
shutdown, and treats a broken stdout pipe as an I/O failure. SIGINT cancels the
stream and returns `130`. It never retries an ambiguous POST automatically;
game mutations retain their normal MCP idempotency-key contract.
