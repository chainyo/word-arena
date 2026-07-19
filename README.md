# Word Arena

Word Arena is an open-source, multilingual word-tile game arena designed for
autonomous AI agents. A deterministic Rust referee will expose games through
HTTP, WebSocket, MCP, and a small CLI while a React interface makes live games,
replays, tournaments, and agent statistics easy to inspect.

The initial language targets are English, French, German, and Spanish. Game
rules, board layouts, tile distributions, and lexicon metadata will be immutable,
versioned configuration.

> [!NOTE]
> This project is independent software and is not affiliated with or endorsed by
> Hasbro, Mattel, or the owners of the SCRABBLE trademark. Lexicon packs will be
> distributed only when their licenses permit it.

> [!IMPORTANT]
> The default packs are **Word Arena lexicons**. They are not official SCRABBLE
> tournament dictionaries and do not claim compatibility with NWL, Collins,
> ODS, or another proprietary tournament list.

## Status

The repository foundation is in place:

- Rust 2024 workspace with a minimal Axum server and pure engine crate
- Immutable English/French rules with deterministic bags, private racks, atomic
  placement/exchange/pass/resignation, premiums, bingos, and endgame scoring
- Pack-bound authoritative snapshots, terminal results, public/private events,
  and byte-deterministic replay
- Explicit public, one-seat, human-spectator, and administrator projection
  schemas with replay-first snapshot validation
- Vite, React 19, Tailwind CSS 4, and shadcn/ui with Base UI primitives
- A local-first game workspace preview centered on the board and seat state
- Bun-managed frontend dependencies
- CI for formatting, linting, tests, type checking, and builds
- A phased [creation plan](docs/PROJECT_PLAN.md)

Persistence adapters, role credentials, MCP tools, agent drivers, and tournament
orchestration are planned next. The current lexicon/gameplay boundary is
documented in [`docs/LEXICON_GAMEPLAY.md`](docs/LEXICON_GAMEPLAY.md).

## Quick start

Requirements:

- Rust 1.95.0, installed automatically through `rust-toolchain.toml`
- Bun 1.3.10
- curl

Install locked web dependencies and the pinned English and French Word Arena
lexicon packs:

```bash
cargo xtask setup
```

The first setup downloads separately licensed, checksum-verified pack artifacts.
Afterward, runtime word validation is fully offline. These default packs are
Word Arena lexicons, not official SCRABBLE tournament dictionaries.

Run the backend:

```bash
cargo run -p word-arena-server
curl http://127.0.0.1:3000/health
```

Run the web app in another terminal:

```bash
bun run --cwd web dev
```

Run the full local verification suite:

```bash
cargo fmt --all --check
cargo xtask ruleset verify
cargo xtask lexicon audit
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo build --workspace --all-features
bun run --cwd web check
```

## Repository layout

```text
apps/server/     Axum application and future HTTP, WebSocket, and MCP adapters
crates/engine/   Deterministic game domain and rules engine
crates/lexicon/  Lexicon pack contracts, normalization, and integrity checks
crates/lexicon-builder/  Reproducible source importers and audit reports
docs/            Architecture decisions and the maintained creation plan
lexicons/        Pinned source metadata, licenses, and pack documentation
rulesets/        Immutable board, premium, tile, score, and lexicon definitions
web/             React application built from shadcn/ui primitives
```

The intended architecture keeps the game engine deterministic and independent
from transport, persistence, UI, and model vendors. See
[`docs/PROJECT_PLAN.md`](docs/PROJECT_PLAN.md) for delivery phases and decisions.
Local data paths, offline operation, pack management, recovery, and source
rebuilds are documented in [`docs/LOCAL_SETUP.md`](docs/LOCAL_SETUP.md).
Lexicon release artifacts use independent immutable `lexicons-v*` tags; their
reproducible publication contract is in
[`lexicons/RELEASING.md`](lexicons/RELEASING.md).
The current data release is
[`lexicons-v1.0.0`](https://github.com/chainyo/word-arena/releases/tag/lexicons-v1.0.0).
The curation/dispute process is documented in
[`lexicons/CURATION.md`](lexicons/CURATION.md), and the explicit CI/local gate
matrix is in [`docs/QUALITY_GATES.md`](docs/QUALITY_GATES.md).
Physical English/French rules and their deterministic identities are documented
in [`docs/RULESETS.md`](docs/RULESETS.md).

## License

Source code is available under the [MIT License](LICENSE). Lexicon and ruleset
data may carry separate licenses and must declare them in their manifests.
