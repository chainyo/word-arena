# ADR 0004: Separate agent-runtime boundary

## Status

Accepted.

## Context

Word Arena needs reproducible configuration, process drivers, workspace
isolation, budgets, and privacy-safe telemetry for multiple coding-agent
harnesses. These concerns do not belong to deterministic Scrabble rules and
must not make the engine depend on provider SDKs, processes, filesystems, or
credentials. Keeping each adapter in the server would also duplicate the
manifest and lifecycle contract.

## Decision

Create `word-arena-agent-runtime` as a game-agnostic crate. Its first public
contract is the strict V1 agent manifest and canonical content identity. The V1
process contract adds typed async lifecycle operations, injected process/time
adapters, stable checkpoints, and privacy-safe normalized telemetry. The first
generic adapter directly executes an argument vector with an empty inherited
environment and uses strict bounded JSON-lines framing. Platform-specific
adapters remain behind the same boundary.

The manifest can identify provider/model/harness versions but cannot represent
provider secrets, game capabilities, operator authority, assigned paths, or
process state. The persistence adapter stores canonical bytes and repeats their
identity at run-result and replay attribution boundaries. The engine replay
schema remains independent; result/export assembly joins attribution outside
the engine.

## Consequences

- One manifest has a stable identity across all drivers and storage adapters.
- Game rules remain deterministic and provider-independent.
- Unknown, unsafe, or secret-bearing configuration fails before execution.
- Process lifecycle and enforcement logic can evolve without widening MCP or
  game-domain APIs.
- Application restart can validate a stable checkpoint and either reattach a
  supervised process or record and start a clean replacement.
- Hidden reasoning is neither requested nor representable in turn telemetry;
  stderr is explicitly an operator-visible diagnostic channel.
- Export code must explicitly join agent attribution when producing tournament
  results or public replay bundles.
