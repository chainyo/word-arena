#!/usr/bin/env bash
set -euo pipefail

if [[ "${WORD_ARENA_RUN_PLATFORM_BUDGET_SMOKE:-}" != "1" ]]; then
  echo "set WORD_ARENA_RUN_PLATFORM_BUDGET_SMOKE=1 to run platform budget checks" >&2
  exit 2
fi

cargo test -p word-arena-agent-runtime --all-features --test budget -- --nocapture
