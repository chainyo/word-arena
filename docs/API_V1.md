# HTTP and WebSocket API V1

All JSON responses are versioned. Successful REST responses use:

```json
{"schema_version":1,"data":{}}
```

Application errors use this strict shape and never include tokens, racks, seed,
bag order, snapshots, or database diagnostics:

```json
{"schema_version":1,"code":"unauthorized","message":"a valid scoped capability is required"}
```

Except for local operator game creation, send capabilities only in the header:

```http
Authorization: Bearer wa_cap_v1.<identifier>.<secret>
```

Role, seat, and capability values are never accepted from a JSON request body or
query as authorization input.

## REST endpoints

### Create a game

`POST /api/v1/games`

```json
{"language":"english","mode":"competitive","idempotency_key":"client-create-1"}
```

The response contains `game_id`, the initial public projection, and one raw
public-observer capability. That token is returned once. Competitive seats and
operator capabilities are provisioned by trusted orchestration, not this
endpoint. French uses `"french"`; German and Spanish remain unsupported until
their offline rules/lexicons ship. `mode` is immutable and may be `competitive`
(the default when omitted) or `practice`; only practice games may issue a
rate-limited preview capability.

### Observe

- `GET /api/v1/games/{game_id}/public` requires `observe_public`.
- `GET /api/v1/games/{game_id}/seat` requires `observe_seat` and returns only
  the capability's fixed seat.
- `GET /api/v1/games/{game_id}/spectator` requires
  `observe_human_spectator` and returns both current racks but no future bag or
  seed.
- `GET /api/v1/games/{game_id}/administrator` requires
  `observe_administrator` and returns the authoritative checkpoint.
- `GET /api/v1/games/{game_id}/rules` requires `observe_public` and returns the
  immutable ruleset with its exact lexicon identity.

Each route maps one scope to one compile-time application credential. A token
for another route returns the same generic unauthorized response.

Role-safe game views also include `observed_at` and the persisted
`turn_deadline` (`turn`, `seat`, `deadline_at`, and policy version) while the
game is active. The deadline is public timing metadata; the server remains the
only authority that resolves expiry.

### Act

`POST /api/v1/games/{game_id}/actions` requires `act`:

```json
{
  "expected_version": 0,
  "turn_number": 0,
  "idempotency_key": "turn-0-attempt-1",
  "action": {"type": "pass"}
}
```

Other action shapes are the engine's tagged `place`, `exchange`, and `resign`
variants. The server derives the acting seat from the authenticated capability.
Unknown fields are rejected. Accepted actions return the public event plus the
updated acting-seat projection, the successor turn deadline, and a public
invalidation marker.

## WebSocket invalidations

Connect with the bearer header to:

```text
GET /api/v1/games/{game_id}/events?after_version=12
```

Browser clients, which cannot set an `Authorization` header on `WebSocket`,
send the protocols `word-arena-v1` and the opaque capability. The server
selects only `word-arena-v1`; the capability must never be placed in the URL or
persisted in browser storage.

Every text message has exactly this public shape:

```json
{"schema_version":1,"game_id":"game-example","version":13}
```

The stream never sends an event payload, rack, score calculation, token, seed,
or bag data. When `version` is newer than the client's snapshot, fetch the
appropriate REST route. Duplicate/older markers may be ignored. On reconnect,
send the last REST version as `after_version`; the server immediately sends the
current marker when behind. A cursor ahead of the server returns HTTP 409.

## Limits and origins

- JSON request bodies: 64 KiB.
- In-flight HTTP requests: 128.
- Open WebSockets: 64.
- HTTP request timeout: 15 seconds.
- WebSocket frame and message size: 8 KiB.
- Browser origins: `http://127.0.0.1:5173` and `http://localhost:5173`.

The local process handles SIGINT/Ctrl-C with graceful Axum shutdown. Runtime
SQLite data and the persistent 32-byte capability HMAC key live under the Word
Arena data directory; the key is created with mode `0600` on Unix.

## Mutation reliability

Create and action idempotency keys are mandatory. Repeating a key with the same
canonical payload returns the original game or exact action outcome; reusing it
with another payload returns HTTP 409. The server persists only key digests.
Accepted action state and its retry outcome commit atomically.

Active turns have persisted deadlines. A restart-safe worker resolves due turns
with the configured pass-or-resign policy. Player actions and timeouts compete
on the same expected version, so only one commits. See
[`RELIABILITY.md`](RELIABILITY.md) for the storage and recovery contract.
