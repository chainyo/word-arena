#!/usr/bin/env bash
set -euo pipefail

for variable in WORD_ARENA_SERVER WORD_ARENA_GAME_ID WORD_ARENA_TOKEN; do
  if [[ -z "${!variable:-}" ]]; then
    echo "missing required environment variable: $variable" >&2
    exit 2
  fi
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cargo build --manifest-path "$repo_root/Cargo.toml" -p word-arena-cli
bridge="$repo_root/target/debug/word-arena"
inspector="@modelcontextprotocol/inspector@0.21.2"

bunx "$inspector" --cli "$bridge" mcp stdio --method tools/list
bunx "$inspector" --cli "$bridge" mcp stdio --method resources/list
bunx "$inspector" --cli "$bridge" mcp stdio \
  --method tools/call \
  --tool-name observe_game \
  --tool-arg schema_version=1
