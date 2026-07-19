# Isolated agent seat workspaces

Every autonomous process runs inside one workspace allocated to exactly one
`run_id` and `seat_id`. The V1 contract is recorded in
[`contracts/agent-workspace-v1.json`](../contracts/agent-workspace-v1.json) and
implemented by `SeatWorkspaceManager` in `word-arena-agent-runtime`.

## Layout and lifecycle

The manager creates private `0700` directories beneath a configured absolute
root. Run and seat identifiers accept only bounded ASCII letters, digits,
hyphens, and underscores. Existing symlinks, permissive modes, ownership drift,
path traversal, and allocation collisions fail closed.

Each seat receives this stable layout:

```text
runs/<run_id>/<seat_id>/
  workspace/       agent files, preserved across turns
  state/           harness state and isolated Codex configuration
  home/            isolated HOME
  tmp/             isolated temporary files
  config/          secret-free MCP and CLI configuration
  workspace.json   immutable run/seat/game/manifest binding
```

`delete_on_finish` removes the narrow seat root after completion and also after
an abandoned lease. `retain_on_failure` preserves it only for a failed or
crashed run. Resume requires the same run, seat, game, manifest identity,
harness, and retention policy; it verifies all managed files before accepting a
new short-lived capability. Agent-created files under the writable directories
remain available across turns and resume.

## Credential boundary

The capability is a trusted runtime input, never manifest data. V1 accepts only
the versioned seat-token shape with a future expiry no more than one hour away.
It is injected as `WORD_ARENA_SEAT_CAPABILITY` into an otherwise empty child
environment. Generated configuration refers to that environment key and never
contains the bearer value. The process receives only its game, run, seat, MCP
URL, managed paths, a fixed safe `PATH`, locale values, and required per-harness
state variables.

Opponent, human-spectator, administrator, database, provider, shell-session,
and arbitrary inherited variables are absent. Raw capability values are
redacted from complete and chunk-split stdout/stderr before driver parsing or
telemetry can observe them. RUN-005 adds an explicit forbidden-authority audit
boundary in addition to this allowlist.

## OS process isolation

Directory permissions alone cannot isolate two processes running as the same
local user. Word Arena therefore launches through a fail-closed OS sandbox:

- macOS uses `/usr/bin/sandbox-exec`, exposing reviewed runtime roots and only
  the current seat root; writes are limited to `workspace`, `state`, `home`,
  and `tmp`;
- Linux uses Bubblewrap with user/session/mount/PID isolation, read-only runtime
  and seat roots, and writable binds only for those four directories;
- a platform without a supported executable returns `SandboxUnavailable`
  instead of starting the agent without isolation.

The manifest network policy is translated at the sandbox boundary. `deny`
keeps the network namespace closed. Detailed endpoint and resource enforcement
is completed by RUN-006.

Before every process spawn, the adapter canonicalizes the working directory
inside its seat and verifies hashes for every managed configuration file.
The dedicated `config` directory is read-only in the process sandbox. A harness
may write its own `state` directory, so any change to the managed Codex config
there prevents the next process from starting.

## Verification

Run the focused suite with:

```bash
cargo test -p word-arena-agent-runtime --all-features --test workspace
```

The suite covers private modes, collisions, traversal, symlinks, invalid and
overlong capabilities, URL credentials, config tampering, stateful resume,
capability rotation, cleanup, retention, inherited-environment removal, output
redaction, and runtime path binding. On supported hosts, two concurrent hostile
shell fixtures attempt direct and symlinked cross-seat reads under the real OS
sandbox and must fail while retaining access to their own writable directory.
