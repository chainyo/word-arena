# Replay and analytics exports

Word Arena V1 publishes deterministic, checksummed JSON records and bounded
JSONL streams. The machine-readable contract is `contracts/exports-v1.json`.

## Record families

The export envelope supports independently versioned public replay, tournament
result, standings, rating, and analytics records. Public and operator replay and
analytics are distinct Rust and wire variants, so a public caller cannot request
complete data by toggling a boolean on the same payload.

Every envelope declares an exact content type, injected generation time,
producer, sorted source identities/checksums, audience policy, and SHA-256. The
checksum covers the schema, content type, provenance, policy, and typed record.
Identical inputs therefore produce byte-identical compact JSON.

## Public replay verification

A public replay contains the complete ruleset, exact lexicon pack identity,
RNG algorithm, post-game seed reveal, and ordered public events. It never embeds
lexicon contents, private events, draws, or rack snapshots. The seed lets the
Rust referee reconstruct bag and rack state internally and replay every public
placement, exchange, pass, and resignation against the referenced installed
pack. Any schema, seed, pack, event, or score substitution fails verification.

Complete `ReplayBundle` exports are operator-only. They include the existing
seat-private transition history for authorized audit, but still never contain
capabilities, provider credentials, or hidden model reasoning.

## Streaming and ordering

`JsonlExporter` accepts one verified envelope at a time and writes compact JSON
plus a newline. Records must arrive in strictly increasing record-family and
stable-identity order; duplicate or reordered records fail. Per-record and
whole-stream byte limits are checked before each write. The final summary gives
the content type, record count, byte count, and SHA-256 over the exact stream.

The default hard ceilings are 16 MiB per record and 1 GiB per stream. Callers
may choose smaller limits. Lexicon data is always referenced by immutable pack
identity rather than copied into any export.

## Privacy policy

Public envelopes are recursively checked for forbidden content-bearing keys and
credential-shaped strings. Their explicit policy records omitted private
events, racks, word frequencies, transcripts, tool payloads, diagnostics,
capabilities, and lexicon contents. Operator envelopes use a separate policy
that permits complete replay and word drill-down while continuing to omit
credentials and hidden reasoning.

```bash
cargo test -p word-arena-engine --all-features --test public_replay
cargo test -p word-arena-application --all-features --test export
```
