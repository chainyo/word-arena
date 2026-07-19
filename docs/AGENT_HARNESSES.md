# Supported agent harnesses

Word Arena V1 supports Codex, Claude Code, Cline, Pi, and a generic command
through one `AgentDriver` interface. The tagged reviewed contract is
`contracts/agent-harnesses-v1.json`; immutable manifests still pin the exact
installed version used by a run.

## Reviewed baselines

The minimums below were reviewed on 2026-07-19. They are compatibility floors,
not floating installation requests:

| Harness | Minimum | Native mode | Primary reference |
| --- | ---: | --- | --- |
| Codex | 0.144.0 | `codex exec --json --ephemeral` | [OpenAI non-interactive mode](https://developers.openai.com/codex/non-interactive) |
| Claude Code | 2.1.205 | print mode with `stream-json` | [Claude Code CLI reference](https://code.claude.com/docs/en/cli-usage) |
| Cline | 3.0.46 | headless `--json` | [Cline CLI reference](https://docs.cline.bot/cli/cli-reference) |
| Pi | 0.73.1 | print mode with JSON events | [Pi quickstart](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/quickstart.md) |

Every native adapter runs `<executable> --version` before accepting a turn. The
reported semantic version must be at least the reviewed minimum and exactly
equal the version in the immutable manifest. Missing executables, malformed
version output, unsupported versions, and exact-version drift are distinct
actionable failures.

## Common lifecycle

`SupportedAgentDriver` selects the generic persistent JSON-lines driver or the
native one-shot driver without changing application code. Native harnesses are
started once per game for compatibility verification and once per turn in
their documented headless mode. Persistent seat workspace and state directories
preserve allowed harness state across those turn processes. Their allocation,
credential injection, and fail-closed OS sandbox are described in
`docs/AGENT_WORKSPACES.md`.

The runtime translates model, provider, workspace, state, MCP configuration,
and tool/network policy from the validated manifest plus trusted per-seat
paths. The exact `HarnessPolicyTranslation` is retained for budget enforcement
added in RUN-006. Commands already select non-interactive structured
output and the strictest applicable built-in network/permission settings. The
generic adapter replaces only the exact `{workspace}`, `{mcp_config}`, and
`{state_directory}` argument placeholders and never invokes a shell.

Native stdout is capped at 1 MiB and decoded according to its harness schema:

- Codex: completed agent messages and MCP tool calls; completion requires
  `turn.completed`.
- Claude Code: assistant text/tool blocks plus a successful `result` event.
- Cline: non-partial MCP events and `completion_result`.
- Pi: assistant `message_end` text/tool blocks plus `agent_end`.

Reasoning/thinking blocks, partial output, and unrelated events are discarded.
Raw native stdout, stderr, prompts, config paths, environment, and commands are
never copied to diagnostics. Stderr is represented only by a byte count and a
redaction code. This differs from the generic protocol, where stderr is an
explicitly operator-visible channel controlled by the custom integration.

## Credentials and configuration

Manifests cannot contain credentials. `ProcessSpec` also has a redacted debug
representation, and every process starts with an empty inherited environment.
The workspace manager supplies only one seat's short-lived capability and
secret-free MCP/CLI configuration. Opponent, spectator, administrator, and
database credentials remain absent from commands, files, and telemetry.

## Verification and local smoke checks

CI uses one executable fake fixture, exposed under all five harness names. It
validates real direct process execution, every command shape, provider output
normalization, restart checkpoints, placeholder binding, hidden-reasoning
discard, and stderr redaction without a network call or paid credential:

```bash
cargo test -p word-arena-agent-runtime --all-features --test harness
```

For local installations, inspect versions without a model call:

```bash
scripts/agents/smoke-harnesses.sh versions
```

A deliberately opt-in live prompt can be run for one harness after its normal
authentication is configured:

```bash
WORD_ARENA_LIVE_AGENT_SMOKE=1 scripts/agents/smoke-harnesses.sh live codex
```

Live smoke calls may consume provider quota and are never run in CI.
