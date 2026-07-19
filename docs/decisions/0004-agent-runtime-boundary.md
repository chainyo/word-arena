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
adapters remain behind the same boundary. Native coding harnesses execute one
structured headless process per turn, after an exact version probe, because
their upstream wire formats differ from the persistent generic protocol. Their
parsers normalize only visible messages and tool calls.

Each run/seat also receives one stable private filesystem tree and an otherwise
empty process environment. A trusted manager validates ownership, modes,
identifiers, symlinks, manifest-bound metadata, and managed configuration on
allocation and resume. Short-lived seat authority is passed only through an
environment key referenced by secret-free config files. Agent processes run in
a platform sandbox (`sandbox-exec` on macOS or Bubblewrap on Linux) that exposes
only reviewed runtime paths and that seat's tree; unsupported platforms fail
closed.

Human-spectator and administrator bearer values stay in the operator issuance
and storage path. Trusted orchestration consumes them into a digest-only,
non-serializable forbidden-authority registry. Allocation and every process
spawn scan all untyped agent inputs against this registry and emit a mandatory
privacy-safe denial audit before failing closed. The registry is only a leak
detector; application credential types remain the authorization boundary.

Resource enforcement is composed as two decorators sharing one controller: a
driver boundary for attempts, normalized tool calls, and deadlines, and a
process boundary for raw output and deadlines. Platform/provider support is a
versioned input to the run. Operators either accept explicitly reported weaker
dimensions or require all-hard support and fail before launch. Direct children
own a process group so every termination path targets descendants as well.

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
- Native harness stderr and raw commands are redacted rather than treated as
  generic operator-visible protocol output.
- Directory permissions are defense in depth; cross-seat confidentiality is
  enforced at the process sandbox boundary for agents sharing one local user.
- Allowed seat state can survive a crash/resume, while managed metadata and
  configuration are reverified before each spawn and tampering fails closed.
- Local deployments require a supported sandbox executable before autonomous
  agent processes can start.
- Operator credentials are not driver configuration or state; only one-way
  leak-detection fingerprints may be retained by the startup guard.
- CPU, memory, token, cost, and network-byte limits must never be presented as
  hard unless a reviewed platform/provider adapter actually supplies them.
- Export code must explicitly join agent attribution when producing tournament
  results or public replay bundles.
