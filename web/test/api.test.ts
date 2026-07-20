import { afterEach, beforeEach, describe, expect, test } from "bun:test"

import contract from "../../contracts/web-api-v1.json"
import {
  fetchGameView,
  GameApiError,
  normalizeServerOrigin,
  snapshotPath,
  submitGameAction,
} from "../src/api/client"
import { credentialVault } from "../src/api/credentials"
import {
  DecodeError,
  decodeAgentCatalog,
  decodeAgentMatchActivity,
  decodeAgentMatchStatus,
  decodeApiError,
  decodeGameView,
  decodeInvalidation,
} from "../src/api/decode"
import { gameQueryKey } from "../src/api/query"
import {
  API_SCHEMA_VERSION,
  type GameAuthority,
  type GameSession,
  PROJECTION_SCHEMA_VERSION,
  REPLAY_SCHEMA_VERSION,
  WEBSOCKET_PROTOCOL,
} from "../src/api/types"
import {
  reconnectDelay,
  shouldInvalidate,
  websocketUrl,
} from "../src/api/websocket"

const session: GameSession = {
  authority: "public",
  gameId: "game-one",
  serverOrigin: "http://127.0.0.1:3000",
}

function publicProjection() {
  return {
    schema_version: 1,
    state: {
      game_id: "game-one",
      ruleset_id: "english-v1",
      mode: "competitive",
      board: Array.from({ length: 225 }, () => null),
      scores: [0, 0],
      current_player: "one",
      version: 3,
      scoreless_turns: 0,
      rack_counts: [7, 7],
      bag_count: 86,
      phase: "active",
    },
    events: [
      {
        sequence: 0,
        visibility: { scope: "public" },
        lexicon: {},
        kind: { type: "created" },
      },
    ],
  }
}

function envelope(authority: GameAuthority): unknown {
  const publicGame = publicProjection()
  const game =
    authority === "public"
      ? publicGame
      : authority === "seat"
        ? {
            schema_version: 1,
            seat: "one",
            public: publicGame,
            rack: [{ id: 7, face: { kind: "letter", token: "A" } }],
            private_events: [],
          }
        : {
            schema_version: 1,
            public: publicGame,
            racks: [
              [{ id: 7, face: { kind: "letter", token: "A" } }],
              [{ id: 8, face: { kind: "blank" } }],
            ],
            private_events: [],
          }
  return {
    schema_version: 1,
    data: {
      observed_at: 1234,
      turn_deadline: {
        turn: 3,
        seat: "one",
        deadline_at: 61_234,
        policy_version: 1,
      },
      game,
    },
  }
}

describe("HTTP V1 decoding and drift", () => {
  test("decodes the local agent catalog and content-free match status", () => {
    expect(
      decodeAgentCatalog({
        schema_version: 1,
        data: [
          {
            id: "codex",
            display_name: "Codex",
            logo: "openai",
            available: true,
            compatible: true,
            version: "0.144.1",
            minimum_version: "0.144.0",
            diagnostic: "Ready",
          },
        ],
      })[0]
    ).toMatchObject({ id: "codex", compatible: true, version: "0.144.1" })

    const status = decodeAgentMatchStatus({
      schema_version: 1,
      data: {
        schema_version: 1,
        game_id: "game-one",
        language: "english",
        mode: "practice",
        phase: "active",
        orchestration: "active",
        version: 3,
        current_seat: "one",
        scores: [12, 8],
        created_at_unix_ms: 1_234,
        updated_at_unix_ms: 2_345,
        seats: [
          {
            seat: "one",
            participant: { kind: "agent", harness: "codex", model: null },
            status: { state: "thinking" },
          },
          {
            seat: "two",
            participant: { kind: "human", name: "Ada" },
            status: { state: "waiting_for_human" },
          },
        ],
      },
    })
    expect(status).toMatchObject({
      language: "english",
      mode: "practice",
      orchestration: "active",
      scores: [12, 8],
    })
    expect(status.seats[0].state).toBe("thinking")
    expect(status.seats[1].participant).toEqual({ kind: "human", name: "Ada" })
  })

  test("decodes the spectator-only agent activity feed", () => {
    const activity = decodeAgentMatchActivity({
      schema_version: 1,
      data: {
        schema_version: 1,
        game_id: "game-one",
        events: [
          {
            sequence: 4,
            at_unix_ms: 12_000,
            seat: "one",
            kind: "turn_started",
            message: "Turn 3 started",
            turn_id: "1-3",
            duration_ms: null,
          },
        ],
      },
    })
    expect(activity.events[0]).toEqual({
      sequence: 4,
      atUnixMs: 12_000,
      seat: "one",
      kind: "turn_started",
      message: "Turn 3 started",
      turnId: "1-3",
      durationMs: undefined,
    })
  })

  test("shares exact schema, route, and WebSocket constants", () => {
    expect(API_SCHEMA_VERSION).toBe(contract.api_schema_version)
    expect(PROJECTION_SCHEMA_VERSION).toBe(contract.projection_schema_version)
    expect(REPLAY_SCHEMA_VERSION).toBe(contract.replay_schema_version)
    expect(WEBSOCKET_PROTOCOL).toBe(contract.browser_websocket_protocol)
    expect(contract.view_fields).toEqual([
      "observed_at",
      "turn_deadline",
      "game",
    ])
    for (const authority of ["public", "seat", "spectator"] as const) {
      expect(snapshotPath({ ...session, authority })).toBe(
        contract.projection_paths[authority].replace("{game_id}", "game-one")
      )
    }
    expect(contract.spectator_replay_path).toBe(
      "/api/v1/games/{game_id}/spectator/replay"
    )
    expect(contract.agent_paths.activity).toBe(
      "/api/v1/matches/{game_id}/activity"
    )
  })

  test("decodes each authority without widening its projection", () => {
    const publicView = decodeGameView(envelope("public"), "public")
    expect(publicView.rack).toBeUndefined()
    expect(publicView.turnDeadline?.deadlineAt).toBe(61_234)
    expect(decodeGameView(envelope("seat"), "seat").rack?.[0]?.id).toBe(7)
    expect(decodeGameView(envelope("seat"), "seat").racks).toBeUndefined()
    expect(
      decodeGameView(envelope("spectator"), "spectator").racks?.[1][0]?.id
    ).toBe(8)
  })

  test("fails closed on schema drift and forbidden private fields", () => {
    const drifted = envelope("public") as Record<string, unknown>
    drifted.schema_version = 2
    expect(() => decodeGameView(drifted, "public")).toThrow(DecodeError)

    const leaked = envelope("public") as {
      data: { game: Record<string, unknown> }
    }
    leaked.data.game.rack = []
    expect(() => decodeGameView(leaked, "public")).toThrow("forbidden rack")

    const opponentLeak = envelope("seat") as {
      data: { game: Record<string, unknown> }
    }
    opponentLeak.data.game.racks = [[], []]
    expect(() => decodeGameView(opponentLeak, "seat")).toThrow(
      "forbidden racks"
    )
  })

  test("decodes stable errors and invalidations", () => {
    expect(
      decodeApiError({ schema_version: 1, code: "unauthorized", message: "no" })
    ).toEqual({ schema_version: 1, code: "unauthorized", message: "no" })
    expect(
      decodeInvalidation({ schema_version: 1, game_id: "game-one", version: 4 })
    ).toEqual({ schema_version: 1, game_id: "game-one", version: 4 })
  })
})

describe("credentials, cache keys, and authentication", () => {
  const originalFetch = globalThis.fetch

  beforeEach(() => credentialVault.clear())
  afterEach(() => {
    globalThis.fetch = originalFetch
    credentialVault.clear()
  })

  test("keeps capabilities out of cache keys and isolates authorities", () => {
    credentialVault.set(session, "wa_cap_v1.public.secret")
    const key = gameQueryKey(session)
    expect(JSON.stringify(key)).not.toContain("secret")
    expect(
      credentialVault.get({ ...session, authority: "seat" })
    ).toBeUndefined()
  })

  test("sends a bearer header but never a capability in the URL", async () => {
    let requestedUrl = ""
    let authorization = ""
    globalThis.fetch = (async (input, init) => {
      requestedUrl = String(input)
      authorization = new Headers(init?.headers).get("authorization") ?? ""
      return new Response(JSON.stringify(envelope("public")), {
        status: 200,
        headers: { "content-type": "application/json" },
      })
    }) as typeof fetch
    await fetchGameView(session, "wa_cap_v1.public.secret")
    expect(requestedUrl).not.toContain("secret")
    expect(authorization).toBe("Bearer wa_cap_v1.public.secret")
  })

  test("surfaces typed authentication failures", async () => {
    globalThis.fetch = (async () =>
      new Response(
        JSON.stringify({
          schema_version: 1,
          code: "unauthorized",
          message: "a valid scoped capability is required",
        }),
        { status: 401 }
      )) as unknown as typeof fetch
    expect(fetchGameView(session, "invalid")).rejects.toBeInstanceOf(
      GameApiError
    )
  })

  test("submits every seat action with version and retry identity", async () => {
    const requests: Array<Record<string, unknown>> = []
    globalThis.fetch = (async (_input, init) => {
      requests.push(JSON.parse(String(init?.body)))
      return new Response(
        JSON.stringify({
          schema_version: 1,
          data: {
            committed_at: 1235,
            turn_deadline: {
              turn: 4,
              seat: "two",
              deadline_at: 61_235,
              policy_version: 1,
            },
            event: { sequence: 4, kind: { type: "passed" } },
            game: {
              schema_version: 1,
              seat: "one",
              public: publicProjection(),
              rack: [],
              private_events: [],
            },
          },
        }),
        { status: 200, headers: { "content-type": "application/json" } }
      )
    }) as typeof fetch
    const seatSession = { ...session, authority: "seat" as const }
    const actions = [
      {
        type: "place" as const,
        placements: [
          {
            tile_id: 1,
            coordinate: { row: 7, column: 7 },
            tile: { letter: "E", is_blank: false },
          },
        ],
      },
      { type: "exchange" as const, tile_ids: [1, 2] },
      { type: "pass" as const },
      { type: "resign" as const },
    ]
    for (const [index, action] of actions.entries()) {
      const view = await submitGameAction(
        seatSession,
        "wa_cap_v1.seat.secret",
        {
          expected_version: 3,
          turn_number: 3,
          idempotency_key: `web-action-${index}`,
          action,
        }
      )
      expect(view.turnDeadline?.turn).toBe(4)
    }
    expect(requests.map((request) => request.action)).toEqual(actions)
    expect(requests.every((request) => request.expected_version === 3)).toBe(
      true
    )
    expect(requests.every((request) => request.turn_number === 3)).toBe(true)
  })
})

describe("reconnect-aware invalidation", () => {
  test("uses bounded exponential delays", () => {
    expect([0, 1, 2, 3, 20].map(reconnectDelay)).toEqual([
      250, 500, 1_000, 2_000, 5_000,
    ])
  })

  test("invalidates only newer markers for the active game", () => {
    expect(
      shouldInvalidate(
        { schema_version: 1, game_id: "game-one", version: 4 },
        session,
        3
      )
    ).toBe(true)
    expect(
      shouldInvalidate(
        { schema_version: 1, game_id: "other", version: 9 },
        session,
        3
      )
    ).toBe(false)
    expect(
      shouldInvalidate(
        { schema_version: 1, game_id: "game-one", version: 3 },
        session,
        3
      )
    ).toBe(false)
  })

  test("reconnects from the last authoritative snapshot version", () => {
    expect(websocketUrl(session, 12)).toBe(
      "ws://127.0.0.1:3000/api/v1/games/game-one/events?after_version=12"
    )
    expect(normalizeServerOrigin("https://arena.local/path?q=1")).toBe(
      "https://arena.local"
    )
  })
})
