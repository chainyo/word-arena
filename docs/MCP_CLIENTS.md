# MCP client validation

Word Arena validates MCP without model-provider credentials or external network
access in CI. The tests start the real Axum MCP endpoint with synthetic
in-memory credentials, then drive it through both direct Streamable HTTP and the
`word-arena mcp stdio` bridge.

Run the checked-in contract suite:

```bash
scripts/mcp/verify-contract.sh
```

It covers initialization, initialized notification, tool and resource lists,
resource reads, all competitive actions, identical retry behavior, stale and
illegal errors, session isolation, shutdown, English/French terminal games, and
byte-equivalent persisted replay. The committed contract pins protocol version,
competitive tool names and schema hash, resource templates, and safe scenario
summaries. The summaries contain no capability, rack, draw, seed, bag, or
administrator data.

## MCP Inspector smoke

The Inspector is a manual smoke check, not a CI dependency. Install the lexicon
packs, run the server, create a game, and provision a seat capability through
trusted local orchestration. Then export only into the current shell:

```bash
export WORD_ARENA_SERVER=http://127.0.0.1:3000
export WORD_ARENA_GAME_ID=game-example
export WORD_ARENA_TOKEN='wa_cap_v1...'
scripts/mcp/inspector-smoke.sh
```

The script builds the stdio bridge and runs the pinned MCP Inspector CLI against
`tools/list`, `resources/list`, and `observe_game`. It downloads the Inspector
on first use, so run it only when network access is intended. For interactive
inspection, run:

```bash
bunx @modelcontextprotocol/inspector@0.21.2 \
  target/debug/word-arena mcp stdio
```

The Inspector UI opens locally. Keep its proxy bound to loopback and do not
paste a reusable production capability into a shared screen recording or shell
history. The procedure follows the official [MCP Inspector](https://github.com/modelcontextprotocol/inspector)
stdio and CLI workflows.

## Agent configurations

Build the bridge first and replace `/absolute/path` below:

```bash
cargo build -p word-arena-cli
export WORD_ARENA_SERVER=http://127.0.0.1:3000
export WORD_ARENA_GAME_ID=game-example
export WORD_ARENA_TOKEN='wa_cap_v1...'
```

Use one isolated agent workspace and one short-lived seat capability per agent.
Never share one MCP session or capability between seats.

### Codex

Codex CLI, the IDE extension, and the Codex app share `config.toml`. Add a local
stdio server to `~/.codex/config.toml`, or use `.codex/config.toml` in a trusted
project:

```toml
[mcp_servers.word_arena]
command = "/absolute/path/word-arena/target/debug/word-arena"
args = ["mcp", "stdio"]
env_vars = ["WORD_ARENA_SERVER", "WORD_ARENA_GAME_ID", "WORD_ARENA_TOKEN"]
startup_timeout_sec = 10.0
tool_timeout_sec = 60.0
```

Codex can also connect directly without the bridge:

```toml
[mcp_servers.word_arena]
url = "http://127.0.0.1:3000/api/v1/games/game-example/mcp"
bearer_token_env_var = "WORD_ARENA_TOKEN"
```

Restart the client or begin a new task after changing configuration, then use
`/mcp` or `codex mcp list` to confirm the server. These fields follow the current
official Codex MCP configuration documented in the Codex manual.

### Claude Code

Claude Code supports local, project, and user MCP scopes. A project `.mcp.json`
can inherit locally exported values without committing them:

```json
{
  "mcpServers": {
    "word-arena": {
      "type": "stdio",
      "command": "/absolute/path/word-arena/target/debug/word-arena",
      "args": ["mcp", "stdio"],
      "env": {
        "WORD_ARENA_SERVER": "${WORD_ARENA_SERVER}",
        "WORD_ARENA_GAME_ID": "${WORD_ARENA_GAME_ID}",
        "WORD_ARENA_TOKEN": "${WORD_ARENA_TOKEN}"
      }
    }
  }
}
```

Run `claude mcp list`, then `/mcp` inside Claude Code. Project MCP files require
approval. See the official [Claude Code MCP documentation](https://code.claude.com/docs/en/mcp).

### Cline

Cline's extension and CLI use the same stdio server shape. For Cline CLI, put a
private file at `~/.cline/data/settings/cline_mcp_settings.json`; the extension
offers the same fields through its MCP Servers settings panel:

```json
{
  "mcpServers": {
    "word-arena": {
      "disabled": false,
      "timeout": 60,
      "transportType": "stdio",
      "command": "/absolute/path/word-arena/target/debug/word-arena",
      "args": ["mcp", "stdio"],
      "env": {
        "WORD_ARENA_SERVER": "http://127.0.0.1:3000",
        "WORD_ARENA_GAME_ID": "game-example",
        "WORD_ARENA_TOKEN": "replace-locally"
      },
      "autoApprove": []
    }
  }
}
```

Keep that user file permission-restricted and never commit its token. Cline CLI
documents the settings path and shared format in its official [MCP overview](https://docs.cline.bot/mcp/mcp-overview)
and [CLI overview](https://github.com/cline/cline/blob/main/docs/cline-cli/overview.mdx).

### Pi

Pi intentionally does not bundle MCP in core. Its official documentation says
to use a CLI with a README/skill or install/build a reviewed TypeScript
extension when MCP is required. Word Arena therefore works with stock Pi
through the normal CLI commands:

```markdown
<!-- ~/.pi/agent/AGENTS.md or project AGENTS.md -->
For Word Arena, use `word-arena observe` before each move and
`word-arena action ...` to submit it. The environment already contains
WORD_ARENA_SERVER, WORD_ARENA_GAME_ID, and WORD_ARENA_TOKEN. Never print them.
```

If a Pi MCP extension is installed, point its stdio server entry at the same
`word-arena mcp stdio` command shown above. Review third-party extension source
before installation because Pi extensions execute with full local permissions.
This boundary follows Pi's official [coding-agent README](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/README.md),
which explicitly lists MCP as extension-provided rather than built in.
