#!/bin/sh
set -eu

mode=${1:-versions}
harness=${2:-all}

show_version() {
  label=$1
  executable=$2
  if command -v "$executable" >/dev/null 2>&1; then
    printf '%s: ' "$label"
    "$executable" --version
  else
    printf '%s: unavailable (%s)\n' "$label" "$executable"
  fi
}

if [ "$mode" = "versions" ]; then
  show_version codex "${WORD_ARENA_CODEX_BIN:-codex}"
  show_version claude_code "${WORD_ARENA_CLAUDE_BIN:-claude}"
  show_version cline "${WORD_ARENA_CLINE_BIN:-cline}"
  show_version pi "${WORD_ARENA_PI_BIN:-pi}"
  if [ -n "${WORD_ARENA_GENERIC_BIN:-}" ]; then
    show_version generic_command "$WORD_ARENA_GENERIC_BIN"
  else
    printf '%s\n' 'generic_command: set WORD_ARENA_GENERIC_BIN to inspect'
  fi
  exit 0
fi

if [ "$mode" != "live" ] || [ "${WORD_ARENA_LIVE_AGENT_SMOKE:-}" != "1" ]; then
  printf '%s\n' 'usage: smoke-harnesses.sh versions' >&2
  printf '%s\n' 'or: WORD_ARENA_LIVE_AGENT_SMOKE=1 smoke-harnesses.sh live <codex|claude_code|cline|pi|generic_command>' >&2
  exit 64
fi

prompt='Reply with exactly WORD_ARENA_READY. Do not read, write, or execute anything.'
case "$harness" in
  codex)
    "${WORD_ARENA_CODEX_BIN:-codex}" --ask-for-approval never exec --ephemeral --sandbox read-only "$prompt"
    ;;
  claude_code)
    "${WORD_ARENA_CLAUDE_BIN:-claude}" --print --permission-mode plan "$prompt"
    ;;
  cline)
    "${WORD_ARENA_CLINE_BIN:-cline}" --plan --auto-approve false "$prompt"
    ;;
  pi)
    "${WORD_ARENA_PI_BIN:-pi}" --print "$prompt"
    ;;
  generic_command)
    if [ -z "${WORD_ARENA_GENERIC_BIN:-}" ]; then
      printf '%s\n' 'WORD_ARENA_GENERIC_BIN is required' >&2
      exit 64
    fi
    "$WORD_ARENA_GENERIC_BIN" --help
    ;;
  *)
    printf '%s\n' 'live smoke requires exactly one supported harness' >&2
    exit 64
    ;;
esac
