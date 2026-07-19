#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

cargo test --manifest-path "$repo_root/Cargo.toml" -p word-arena-server \
  --test transport --all-features web_api_contract_matches_authoritative_server_constants
bun run --cwd "$repo_root/web" test
