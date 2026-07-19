# Agent process drivers

Word Arena drives every autonomous entrant through the `AgentDriver` contract
in `word-arena-agent-runtime`. The driver is intentionally game-agnostic: the
application decides when a seat has a turn and supplies only the visible turn
input; the engine and MCP boundary remain the authoritative referee.

The versioned public constants and the reviewed summary in
`contracts/agent-driver-v1.json` define V1. `GenericCommandDriver` owns the
persistent Word Arena JSON-lines protocol. `SupportedAgentDriver` also
normalizes Codex, Claude Code, Cline, and Pi native headless modes into this
same lifecycle without adding game-specific behavior. See
`docs/AGENT_HARNESSES.md`.

## Lifecycle

The stable states are `pending`, `ready`, `crashed`, and `terminated`. Starting,
turn execution, and termination use explicit transient states. Invalid
transitions fail without touching the process. Cancellation always wins a
simultaneously ready process operation, terminates the process, clears partial
frames, and produces `terminated/cancelled`.

`checkpoint()` is available only in a stable state. A ready checkpoint includes
the opaque process handle; its manifest identity, command, run ID, lifecycle,
and telemetry are all validated by `restore()`. An injected supervisor may
reattach that handle. When a handle is missing or the local Tokio adapter cannot
reattach after application restart, `resume()` records a visible diagnostic and
starts a clean replacement while incrementing the restart count. A crashed
driver also resumes with a clean process and decoder.

## Generic JSON-lines protocol

The driver directly executes the validated executable and argument vector. It
does not invoke a shell and the Tokio adapter clears the inherited environment.
Workspace and credential injection remain separate trusted runtime concerns.

Each request is one UTF-8 JSON object followed by a newline:

```json
{"schema_version":1,"type":"turn","run_id":"run-42","turn_id":"turn-7","visible_input":"It is your turn."}
```

The agent returns exactly one newline-terminated JSON object:

```json
{"schema_version":1,"turn_id":"turn-7","visible_output":"Placed ETE.","tool_calls":[{"tool":"word_arena.place_tiles","arguments":{"tiles":"ETE"},"result":{"accepted":true}}]}
```

Stdout frames are bounded to 1 MiB. Partial reads are assembled, while multiple,
trailing, oversized, malformed, unknown-field, wrong-version, and wrong-turn
responses fail closed and terminate the process. Stderr is an explicitly
visible diagnostic channel and is stored as structured diagnostics, never as a
turn transcript.

## Privacy and telemetry

The output schema accepts only `visible_output` and visible `tool_calls`.
Unknown fields are rejected, so fields such as `reasoning` or
`chain_of_thought` cannot enter turn telemetry. The driver never requests hidden
reasoning. Harnesses must put only operator-visible diagnostics on stderr.

Telemetry contains the exact manifest identity, ordered lifecycle transitions,
restart count, visible turn inputs and outputs, visible tool calls, timings, and
structured diagnostics. Raw partial stdout and invalid frame content are
discarded and never copied into diagnostics.

## Verification

Run the focused contract suite with:

```bash
cargo test -p word-arena-agent-runtime --all-features
```

The fake-process suite covers a synthetic multi-turn match, checkpoint JSON
round-trip and reattachment, every stable transition, cancellation races,
partial frames, stderr, process exit mapping, crash recovery, strict privacy,
frame bounds, and idempotent termination. A Unix smoke test also exercises the
Tokio direct-process adapter without network or credentials.

Production drivers are decorated by `BudgetedAgentDriver` and use a
`BudgetedProcessAdapter` around the isolated seat adapter. This keeps budget
semantics uniform across persistent generic and one-shot native harnesses. See
`docs/AGENT_BUDGETS.md` for platform support and normalized limit telemetry.
