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
contract is the strict V1 agent manifest and canonical content identity. Later
Phase 5 tasks add process-driver, sandbox, budget, and telemetry contracts here
while platform-specific adapters remain behind injected boundaries.

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
- Export code must explicitly join agent attribution when producing tournament
  results or public replay bundles.
