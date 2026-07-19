#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

cargo test --manifest-path "$repo_root/Cargo.toml" \
  -p word-arena-server --test transport --all-features mcp
cargo test --manifest-path "$repo_root/Cargo.toml" \
  -p word-arena-cli --all-features
