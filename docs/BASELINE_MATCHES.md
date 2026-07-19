# Baseline bots and whole-match verification

GAME-006 adds deterministic verification tooling behind the engine crate's
opt-in `test-support` Cargo feature. Default engine and server builds do not
compile this module, and competitive HTTP or MCP transports must never expose
its move generator or greedy scoring choice.

## Boundary

`word_arena_engine::test_support` contains:

- `MoveGenerator`, which takes a small caller-owned candidate catalog, derives
  physical placements, and asks the authoritative engine to validate and score
  each candidate without mutation;
- `BotStrategy::RandomLegal`, which hashes a stable bot seed, game ID, version,
  and seat to select from the sorted legal action set;
- `BotStrategy::Greedy`, which selects the highest immediate legal score and
  breaks ties by the stable physical action key;
- `run_match`, a bounded in-memory runner that creates, deals, plays, finishes,
  snapshots, resumes, and replays a game using public engine transitions.

Pass is always a generated fallback. A one-tile exchange is included only when
the ruleset permits it. Rack-derived placement probes are explicitly opt-in and
exist for broad state-machine tests using a fixture validator; real pack
scenarios use candidate words and the exact immutable pack identity.

## Verification

`crates/engine/tests/baseline_matches.rs` uses small hand-authored English and
French catalogs. It does not contain or derive a third-party dictionary. The
tests cover:

- pinned complete English and French golden outcomes;
- random-legal versus random-legal and greedy versus random-legal in both
  languages;
- byte-equivalent result, snapshot, replay, public, seat, and human-spectator
  artifacts for identical inputs;
- exact V1 pack identity rejection, public/private event visibility, event
  sequencing, scoring, physical tile counts, snapshot resume, and replay;
- resignation, scoreless-turn, and rack-empty terminal rules;
- generated state-machine cases across seeds, languages, and bot matchups;
- 1,000 bounded deterministic complete games, including placement-heavy probe
  games, with authoritative resume/replay checks after every completion.

Run the focused suite with:

```bash
cargo test -p word-arena-engine --all-features --test baseline_matches
```

The full repository gates use `--all-features`, so CI always compiles and runs
this test-only boundary even though production builds leave it disabled.
