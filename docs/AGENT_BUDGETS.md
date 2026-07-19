# Agent resource budgets

Word Arena applies one immutable manifest budget through a shared
`BudgetController`. The reviewed V1 dimensions, support states, normalized
telemetry, and termination behavior are pinned in
[`contracts/agent-budget-v1.json`](../contracts/agent-budget-v1.json).

## Enforcement layers

`BudgetedAgentDriver` decorates any `AgentDriver`, so Codex, Claude Code, Cline,
Pi, and generic command agents share identical turn-attempt, visible tool-call,
and wall-deadline behavior. `BudgetedProcessAdapter` wraps the isolated seat
adapter and counts raw stdout plus stderr before any harness parser can buffer
it. The same run-scoped controller must be passed to both decorators.

Every direct Tokio child starts a new process group. Termination for a budget,
timeout, external cancellation, game end, or operator action kills that whole
group and waits for the supervised child. The macOS/Linux workspace sandbox is
still the credential and filesystem boundary; on Linux, Bubblewrap's
`--die-with-parent` extends termination into its namespace.

V1 enforcement is explicit:

| Dimension | V1 status | Source |
| --- | --- | --- |
| Wall time | hard | monotonic Tokio deadline around driver and process waits |
| Attempts | hard | common `request_turn` boundary |
| Tool calls | hard | normalized visible tool calls |
| Output bytes | hard | raw stdout/stderr stream meter |
| Network bytes | hard only for `deny` (zero access) | OS sandbox; MCP/allowlist byte metering is unavailable |
| Input/output tokens | conditional | exact provider/runtime usage when supplied |
| Cost | conditional | exact provider/runtime usage when supplied |
| CPU time | unenforced | no reviewed portable per-process limiter in V1 |
| Memory | unenforced | no reviewed portable per-process limiter in V1 |

`PlatformBudgetCapabilities` records all ten states and their sources before a
run. `allow_reported` permits a run while preserving the weaker statuses.
`fail_closed` rejects any report that is not hard for every dimension. There is
no silent claim that a manifest number is enforced when the current platform or
provider cannot measure it.

Provider adapters may call `record_reported_usage` only for exact exposed
values. Missing values remain conditional rather than estimated. Network
adapters may call `record_network_bytes` when a reviewed meter is installed.

## Telemetry and failure

`BudgetTelemetry` contains the immutable capability report, saturating counters,
and ordered `BudgetLimitEvent` records. Each event identifies dimension, limit,
observed value, source, sequence, and injected-clock timestamp. It contains no
prompt, transcript, rack, credential, command, or environment data.

A limit race resolves before another process event is exposed. The wrapper
terminates the process group and returns a stable `BudgetExceeded` error;
drivers use the distinct `budget_exceeded` terminal reason. Repeated accounting
uses checked/saturating arithmetic and cannot wrap a counter back below a limit.

After the run result becomes terminal,
`SqliteAgentAttributionRepository::record_budget_telemetry` stores exactly one
schema-pinned snapshot under the same run and manifest identity. Foreign keys
reject nonterminal or substituted runs; loading reparses the strict JSON and
revalidates schema versions and limit-event ordering.

The final privacy-safe run archive records exposed usage/cost availability and
retention separately. Expiring a detailed archive also removes its budget
snapshot transactionally; see `docs/AGENT_TELEMETRY.md`.

## Verification

```bash
cargo test -p word-arena-agent-runtime --all-features --test budget
WORD_ARENA_RUN_PLATFORM_BUDGET_SMOKE=1 scripts/agents/smoke-budgets.sh
```

The normal test covers capability reporting, strict-policy rejection,
attempt/tool/output/token/cost accounting, output floods, deadline races, and a
real shell with a background child. The opt-in command repeats the platform
suite and asserts CPU, memory, and non-denied network-byte limits remain
explicitly unenforced until a reviewed local limiter is added.
