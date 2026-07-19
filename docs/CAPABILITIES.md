# Capability security contract

Word Arena authenticates transport clients with opaque bearer capabilities and
maps successful authentication to the unforgeable application credential types.
The engine never parses a token and request bodies never select a role or seat.

## Token and digest format

Production issuance obtains 16 random identifier bytes and 32 random bearer
secret bytes directly from the operating system through `getrandom`. The one
returned token has the versioned form:

```text
wa_cap_v1.<32 lowercase hex identifier>.<64 lowercase hex secret>
```

`CapabilityToken` is neither cloneable nor serializable and redacts its debug
output. The raw string is exposed only by consuming the issuance result. SQLite
stores the public identifier and a 32-byte `HMAC-SHA-256` digest under an
injected server key. The raw token is never passed to the repository.

The digest contract has an explicit version. Authentication loads by public
identifier and verifies the digest with RustCrypto's constant-time
`verify_slice`; malformed, unknown, wrong-key, expired, revoked, cross-game, and
wrong-scope tokens all return the same external unauthorized error.

The direct dependencies are deliberately small and permissively licensed:
[`getrandom` 0.4.3](https://docs.rs/getrandom/0.4.3/getrandom/) and
[`hmac` 0.13.0](https://docs.rs/hmac/0.13.0/hmac/) are both MIT or Apache-2.0.

## Binding and scopes

Every record fixes its game, role and optional seat, sorted unique scopes,
issuance time, exclusive expiry time, and optional agent run. An agent-run link
is accepted only for a competitive seat and is foreign-keyed when persisted.

Allowed scopes are closed by role:

- public: public observation;
- competitive seat: public observation, its own seat observation, and actions;
- human spectator: public and human-spectator observation;
- administrator: public and administrator observation.

The authenticated result contains exactly one typed application credential.
Cross-seat commands remain rejected by the application service even after a
valid seat token is authenticated.

## Rotation, revocation, and auditing

Revocation changes exactly one capability immediately. Rotation atomically
revokes the prior record, inserts a fresh independently generated token digest,
and appends both audits. Other seat capabilities remain unchanged.

Issuance, every authentication outcome, revocation, rotation, and successful
privileged authentication append structured audits. Audit records contain only
the public capability ID, game, role/seat, requested scope, outcome, and time.
They cannot contain a bearer token, rack, future bag, seed, or game snapshot.

When human-spectator or administrator capabilities are active alongside local
autonomous agents, trusted orchestration consumes each raw token once to build
the digest-only `ForbiddenAuthorityPolicy`. The raw operator credential remains
in the human delivery/storage path; only its SHA-256 fingerprint crosses into
the agent-startup guard. Any matching token found in an agent environment,
argument, or workspace is denied and audited without storing either the token
or fingerprint. This runtime guard supplements the server's keyed persistence
digest; it is not an authentication database or an alternative issuance path.

Run the focused verification with:

```bash
cargo test -p word-arena-application --all-features --test capabilities
cargo test -p word-arena-persistence --all-features --test capabilities
```
