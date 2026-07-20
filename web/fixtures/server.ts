const HOSTNAME = "127.0.0.1"
const PORT = 4174
const NOW = 1_735_689_600_000

type Authority = "public" | "seat" | "spectator"
type Scenario = {
  gameId: string
  phase: "active" | "finished"
  reconnect?: boolean
}

const scenarioFixtures: Scenario[] = [
  { gameId: "player-active", phase: "active" },
  { gameId: "spectator-live", phase: "active" },
  { gameId: "reconnect-game", phase: "active", reconnect: true },
  { gameId: "replay-game", phase: "finished" },
  { gameId: "terminal-game", phase: "finished" },
  { gameId: "auth-failure", phase: "active" },
  { gameId: "privacy-game", phase: "active" },
  { gameId: "created-game", phase: "active" },
]
const scenarios = new Map<string, Scenario>(
  scenarioFixtures.map((scenario) => [scenario.gameId, scenario])
)

const versions = new Map<string, number>()
const websocketConnections = new Map<string, number>()

const corsHeaders = {
  "Access-Control-Allow-Headers": "Authorization, Content-Type",
  "Access-Control-Allow-Methods": "GET, POST, OPTIONS",
  "Access-Control-Allow-Origin": "http://127.0.0.1:4173",
  "Cache-Control": "no-store",
}

function json(value: unknown, status = 200) {
  return Response.json(value, { status, headers: corsHeaders })
}

function apiError(status: number, code: string, message: string) {
  return json({ schema_version: 1, code, message }, status)
}

function boardWithWord(finished: boolean) {
  const board = Array.from({ length: 225 }, () => null) as Array<null | {
    tile_id: number
    letter: string
    is_blank: boolean
  }>
  board[112] = { tile_id: 20, letter: "E", is_blank: false }
  if (finished) {
    board[113] = { tile_id: 21, letter: "T", is_blank: false }
    board[114] = { tile_id: 22, letter: "E", is_blank: false }
  }
  return board
}

function events(finished: boolean) {
  const values: Array<Record<string, unknown>> = [
    {
      sequence: 0,
      visibility: { scope: "public" },
      kind: { type: "created", rack_counts: [7, 7], bag_count: 86 },
    },
    {
      sequence: 1,
      visibility: { scope: "public" },
      kind: {
        type: "move_played",
        player: "one",
        placements: [
          {
            tile_id: 20,
            coordinate: { row: 7, column: 7 },
            tile: { letter: "E", is_blank: false },
          },
        ],
        words: [{ text: "E" }],
        bingo_bonus: 0,
        score: 2,
        scores_after: [2, 0],
        rack_counts_after: [7, 7],
        bag_count_after: 85,
        next_player: "one",
        result: null,
      },
    },
  ]
  if (finished) {
    values.push(
      {
        sequence: 2,
        visibility: { scope: "public" },
        kind: {
          type: "move_played",
          player: "one",
          placements: [
            {
              tile_id: 21,
              coordinate: { row: 7, column: 8 },
              tile: { letter: "T", is_blank: false },
            },
            {
              tile_id: 22,
              coordinate: { row: 7, column: 9 },
              tile: { letter: "E", is_blank: false },
            },
          ],
          words: [{ text: "ETE" }],
          bingo_bonus: 0,
          score: 8,
          scores_after: [10, 0],
          rack_counts_after: [5, 7],
          bag_count_after: 83,
          next_player: "two",
          result: null,
        },
      },
      {
        sequence: 3,
        visibility: { scope: "public" },
        kind: {
          type: "resigned",
          player: "two",
          result: { scores: [10, 0] },
        },
      }
    )
  }
  return values
}

function publicProjection(scenario: Scenario) {
  const finished = scenario.phase === "finished"
  return {
    schema_version: 1,
    state: {
      game_id: scenario.gameId,
      ruleset_id: "english-v1",
      mode: "competitive",
      board: boardWithWord(finished),
      scores: finished ? [10, 0] : [2, 0],
      current_player: finished ? "two" : "one",
      version: versions.get(scenario.gameId) ?? (finished ? 5 : 3),
      scoreless_turns: finished ? 1 : 0,
      rack_counts: finished ? [5, 7] : [7, 7],
      bag_count: finished ? 83 : 85,
      phase: scenario.phase,
    },
    events: events(finished),
  }
}

function rack(start: number, letters: string[]) {
  return letters.map((token, index) => ({
    id: start + index,
    face: token === "?" ? { kind: "blank" } : { kind: "letter", token },
  }))
}

function gameView(scenario: Scenario, authority: Authority) {
  const publicGame = publicProjection(scenario)
  const game =
    authority === "public"
      ? publicGame
      : authority === "seat"
        ? {
            public: publicGame,
            seat: "one",
            rack: rack(100, ["E", "T", "A", "R", "I", "N", "?"]),
          }
        : {
            public: publicGame,
            racks: [
              rack(100, ["E", "T", "A", "R", "I", "N", "?"]),
              rack(200, ["S", "O", "L", "D", "U", "C", "H"]),
            ],
          }
  return {
    schema_version: 1,
    data: {
      observed_at: NOW,
      turn_deadline:
        scenario.phase === "active"
          ? {
              turn: publicGame.state.version,
              seat: "one",
              deadline_at: NOW + 300_000,
              policy_version: 1,
            }
          : null,
      game,
    },
  }
}

const lexicon = {
  pack_id: "word-arena-lexicon-en",
  pack_version: "1.0.0",
  format_version: 1,
  locale: "en",
  normalization: { profile: "physical-latin-v1" },
  content_sha256: "b".repeat(64),
}

const ruleset = {
  schema_version: 1,
  id: "english-v1",
  language: "english",
  lexicon,
  game: {
    board: {
      width: 15,
      height: 15,
      squares: Array.from({ length: 225 }, (_, index) => ({
        coordinate: { row: Math.floor(index / 15), column: index % 15 },
        premium:
          index === 112
            ? "double_word"
            : index === 0
              ? "triple_word"
              : "normal",
      })),
    },
    rack_capacity: 7,
    bingo_bonus: 50,
    exchange_minimum: 7,
    scoreless_turn_limit: 6,
    tiles: [
      { face: { kind: "letter", token: "A" }, count: 9, value: 1 },
      { face: { kind: "letter", token: "E" }, count: 12, value: 1 },
      { face: { kind: "letter", token: "T" }, count: 6, value: 1 },
      { face: { kind: "blank" }, count: 2, value: 0 },
    ],
  },
}

function replay(scenario: Scenario) {
  return {
    schema_version: 1,
    data: {
      observed_at: NOW,
      replay: {
        schema_version: 3,
        ruleset_identity: {
          schema_version: 1,
          ruleset_id: "english-v1",
          content_sha256: "a".repeat(64),
        },
        ruleset,
        lexicon,
        rng_algorithm: "xoshiro256-star-star-v1",
        seed_reveal: Array.from({ length: 32 }, (_, index) => index),
        events: events(true),
        private_events: [
          {
            sequence: 2,
            seat: "one",
            removed_tile_ids: [21, 22],
            drawn_tile_ids: [110, 111],
          },
        ],
      },
      game_id: scenario.gameId,
    },
  }
}

function bearer(request: Request) {
  return request.headers.get("Authorization")?.replace(/^Bearer /, "")
}

function validToken(authority: Authority, token: string | undefined) {
  const expected =
    authority === "seat"
      ? "seat-token"
      : authority === "spectator"
        ? "spectator-token"
        : "public-token"
  return token === expected
}

type SocketData = { gameId: string; reconnect: boolean }

const agentCatalog = [
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
  {
    id: "claude_code",
    display_name: "Claude Code",
    logo: "claude",
    available: true,
    compatible: true,
    version: "2.1.205",
    minimum_version: "2.1.205",
    diagnostic: "Ready",
  },
  ...["cline", "pi"].map((id) => ({
    id,
    display_name: id === "cline" ? "Cline" : "Pi",
    logo: id,
    available: false,
    compatible: false,
    version: null,
    minimum_version: id === "cline" ? "3.0.46" : "0.73.1",
    diagnostic: "Not installed",
  })),
]

type FixtureSeatSelection =
  | { kind: "agent"; harness: string; model?: string }
  | { kind: "human"; name: string }

const defaultMatchSeats: [FixtureSeatSelection, FixtureSeatSelection] = [
  { kind: "agent", harness: "codex" },
  { kind: "agent", harness: "claude_code" },
]
const matchSeats = new Map<
  string,
  [FixtureSeatSelection, FixtureSeatSelection]
>()

function agentMatchStatus(gameId: string) {
  const seats = matchSeats.get(gameId) ?? defaultMatchSeats
  return {
    schema_version: 1,
    game_id: gameId,
    phase: "active",
    version: versions.get(gameId) ?? 3,
    current_seat: "one",
    seats: seats.map((participant, index) => ({
      seat: index === 0 ? "one" : "two",
      participant,
      status: {
        state:
          participant.kind === "human"
            ? "waiting_for_human"
            : index === 0
              ? "thinking"
              : "ready",
      },
    })),
  }
}

const server = Bun.serve<SocketData>({
  hostname: HOSTNAME,
  port: PORT,
  async fetch(request, server) {
    if (request.method === "OPTIONS") {
      return new Response(null, { status: 204, headers: corsHeaders })
    }
    const url = new URL(request.url)
    if (request.method === "GET" && url.pathname === "/health") {
      return json({ status: "ok" })
    }
    if (request.method === "GET" && url.pathname === "/api/v1/agents") {
      return json({ schema_version: 1, data: agentCatalog })
    }
    if (request.method === "POST" && url.pathname === "/api/v1/matches") {
      const scenario = scenarios.get("created-game") as Scenario
      const payload = (await request.json()) as {
        seats: [FixtureSeatSelection, FixtureSeatSelection]
      }
      matchSeats.set(scenario.gameId, payload.seats)
      const human = payload.seats.some((seat) => seat.kind === "human")
      return json({
        schema_version: 1,
        data: {
          game_id: scenario.gameId,
          public: publicProjection(scenario),
          public_capability: "public-token",
          spectator_capability: "spectator-token",
          human_capability: human ? "seat-token" : null,
          status: agentMatchStatus(scenario.gameId),
        },
      })
    }
    const agentMatch = url.pathname.match(/^\/api\/v1\/matches\/([^/]+)$/)
    if (request.method === "GET" && agentMatch) {
      if (!bearer(request)) {
        return apiError(401, "invalid_capability", "Capability is invalid")
      }
      return json({
        schema_version: 1,
        data: agentMatchStatus(decodeURIComponent(agentMatch[1] as string)),
      })
    }
    if (request.method === "POST" && url.pathname === "/api/v1/games") {
      const scenario = scenarios.get("created-game") as Scenario
      return json(
        {
          schema_version: 1,
          data: {
            game_id: scenario.gameId,
            public: publicProjection(scenario),
            public_capability: "public-token",
            spectator_capability: "spectator-token",
          },
        },
        201
      )
    }

    const match = url.pathname.match(
      /^\/api\/v1\/games\/([^/]+)\/(public|seat|spectator|rules|actions|events)(?:\/(replay))?$/
    )
    if (!match) return apiError(404, "not_found", "Fixture route not found")
    const gameId = decodeURIComponent(match[1] as string)
    const resource = match[2] as string
    const scenario = scenarios.get(gameId)
    if (!scenario) return apiError(404, "game_not_found", "Game not found")

    if (resource === "events") {
      const protocols = request.headers
        .get("Sec-WebSocket-Protocol")
        ?.split(",")
        .map((value) => value.trim())
      const authorized = protocols?.some((token) =>
        ["public-token", "seat-token", "spectator-token"].includes(token)
      )
      if (!authorized) {
        return apiError(401, "invalid_capability", "Capability is invalid")
      }
      const upgraded = server.upgrade(request, {
        data: { gameId, reconnect: Boolean(scenario.reconnect) },
        headers: { "Sec-WebSocket-Protocol": "word-arena-v1" },
      })
      return upgraded
        ? undefined
        : apiError(500, "upgrade_failed", "WebSocket upgrade failed")
    }

    if (gameId === "auth-failure" || bearer(request) === "expired-token") {
      return apiError(401, "invalid_capability", "Capability is invalid")
    }

    if (resource === "rules") {
      if (!bearer(request)) {
        return apiError(401, "invalid_capability", "Capability is invalid")
      }
      return json({ schema_version: 1, data: ruleset })
    }

    if (resource === "actions") {
      if (request.method !== "POST" || bearer(request) !== "seat-token") {
        return apiError(403, "seat_required", "A seat capability is required")
      }
      versions.set(gameId, 4)
      const result = gameView(scenario, "seat")
      const game = result.data.game as Record<string, unknown>
      const projection = game.public as ReturnType<typeof publicProjection>
      projection.state.current_player = "two"
      projection.state.board[113] = {
        tile_id: 100,
        letter: "E",
        is_blank: false,
      }
      game.rack = rack(101, ["T", "A", "R", "I", "N", "?", "S"])
      return json({
        schema_version: 1,
        data: {
          committed_at: NOW + 1_000,
          turn_deadline: result.data.turn_deadline,
          event: { sequence: 2, kind: { type: "move_played" } },
          game,
        },
      })
    }

    const authority = resource as Authority
    if (!validToken(authority, bearer(request))) {
      return apiError(401, "invalid_capability", "Capability is invalid")
    }
    if (match[3] === "replay") {
      if (authority !== "spectator" || scenario.phase !== "finished") {
        return apiError(409, "game_active", "Replay is available after finish")
      }
      return json(replay(scenario))
    }
    return json(gameView(scenario, authority))
  },
  websocket: {
    open(socket) {
      const { gameId, reconnect } = socket.data
      const count = (websocketConnections.get(gameId) ?? 0) + 1
      websocketConnections.set(gameId, count)
      if (!reconnect) return
      if (count === 1) {
        setTimeout(() => {
          versions.set(gameId, 4)
          socket.close(1012, "deterministic reconnect fixture")
        }, 750)
      } else {
        setTimeout(() => {
          socket.send(
            JSON.stringify({ schema_version: 1, game_id: gameId, version: 4 })
          )
        }, 100)
      }
    },
    message() {},
  },
})

console.info(`Word Arena deterministic fixtures on ${server.url}`)
