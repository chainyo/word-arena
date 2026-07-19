# Agent manifest V1

An agent manifest is the immutable, secret-free attribution input for one
runner configuration. It identifies what was intended to execute; trusted
runtime state such as capabilities, provider credentials, assigned workspace
paths, and process IDs stays outside it.

`word-arena-agent-runtime` parses V1 with unknown-field rejection, validates all
semantic inputs, emits compact canonical JSON, and calculates
`sha256-canonical-json-v1`. Object keys, map keys, and set-valued tool/host lists
have stable ordering. Any model, prompt, harness, environment, policy, budget,
workspace, driver, or label change therefore produces a different identity.
The published enum/canonicalization surface is pinned in
`contracts/agent-manifest-v1.json` and checked against runtime constants.

## Contract

Every manifest requires:

- `schema_version: 1` and a bounded display `name`;
- one tagged `harness` (`codex`, `claude_code`, `cline`, `pi`, or
  `generic_command`) with one exact semantic version;
- a model ID and exactly one tagged source: `harness_default`, a named
  `provider`, or a supported local runtime;
- a prompt format version and SHA-256 digest, never prompt bytes;
- disjoint allowed/denied tool sets and deny, MCP-only, or host-allowlisted
  network policy;
- an OCI image pinned by digest and a normalized platform;
- the exact Word Arena driver version;
- workspace persistence, retention, and size policy without a caller-selected
  filesystem path; and
- positive wall, CPU, memory, network, token, attempt, tool, output, and cost
  budgets.

Known harness adapters construct their commands in trusted driver code.
`generic_command` is the only manifest variant with an executable and argument
vector. It always uses direct process execution: shell executables, shell
substitution/control syntax, control characters, and secret-bearing arguments
are rejected before a process starts.

Tagged model sources make provider and local settings mutually exclusive.
Provider settings contain only a provider enum and model ID. Keys or values
that resemble API keys, passwords, bearer values, credentials, or tokens fail
before deserialization/persistence, and unknown fields fail closed.

## Persistence and replay attribution

`agent_manifests` stores the canonical bytes under their SHA-256 address.
`agent_runs` references that address. Migration 5 repeats the same digest in
`agent_run_results` and `game_replay_agents`; composite foreign keys bind replay
attribution to the original run, game, and seat. The SQLx adapter revalidates
canonical bytes and their digest on load and rejects substitutions.

Engine replay bundles remain game-domain artifacts and do not depend on the
agent runtime crate. Operator/public result assembly can join the immutable
per-seat attribution by `(game_id, replay_version)` without copying manifests
into every engine event.

## Examples and verification

Reviewed examples for all supported harnesses are under `examples/agents/`:

- `codex-v1.json`
- `claude-code-v1.json`
- `cline-v1.json`
- `pi-v1.json`
- `generic-command-v1.json`

They are configuration examples, not minimum-version claims; adapter version
support is pinned by RUN-003. Validate the contract and persistence boundary:

```bash
cargo test -p word-arena-agent-runtime --all-features
cargo test -p word-arena-persistence --all-features --test agent_attribution
```
