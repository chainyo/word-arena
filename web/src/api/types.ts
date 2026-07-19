export const API_SCHEMA_VERSION = 1
export const PROJECTION_SCHEMA_VERSION = 1
export const WEBSOCKET_PROTOCOL = "word-arena-v1"

export type GameAuthority = "public" | "seat" | "spectator"
export type Seat = "one" | "two"
export type GamePhase = "active" | "finished"

export type BoardTile = {
  tile_id: number
  letter: string
  is_blank: boolean
}

export type PhysicalTile = {
  id: number
  face: { kind: "blank" } | { kind: "letter"; token: string }
}

export type GameEvent = {
  sequence: number
  kind: { type: string } & Record<string, unknown>
}

export type PublicGameState = {
  game_id: string
  ruleset_id: "english-v1" | "french-v1"
  mode: "competitive" | "practice"
  board: Array<BoardTile | null>
  scores: [number, number]
  current_player: Seat
  version: number
  scoreless_turns: number
  rack_counts: [number, number]
  bag_count: number
  phase: GamePhase
}

export type PublicProjection = {
  schema_version: number
  state: PublicGameState
  events: GameEvent[]
}

export type GameView = {
  authority: GameAuthority
  observedAt: number
  public: PublicProjection
  turnDeadline?: {
    turn: number
    seat: Seat
    deadlineAt: number
    policyVersion: number
  }
  seat?: Seat
  rack?: PhysicalTile[]
  racks?: [PhysicalTile[], PhysicalTile[]]
}

export type GameInvalidation = {
  schema_version: number
  game_id: string
  version: number
}

export type Coordinate = { row: number; column: number }
export type Premium =
  | "normal"
  | "double_letter"
  | "triple_letter"
  | "double_word"
  | "triple_word"

export type Ruleset = {
  schema_version: number
  id: "english-v1" | "french-v1"
  language: "english" | "french"
  game: {
    board: {
      width: number
      height: number
      squares: Array<{ coordinate: Coordinate; premium: Premium }>
    }
    rack_capacity: number
    bingo_bonus: number
    exchange_minimum: number
    scoreless_turn_limit: number
    tiles: Array<{
      face: { kind: "blank" } | { kind: "letter"; token: string }
      count: number
      value: number
    }>
  }
}

export type Placement = {
  tile_id: number
  coordinate: Coordinate
  tile: { letter: string; is_blank: boolean }
}

export type GameMove =
  | { type: "place"; placements: Placement[] }
  | { type: "exchange"; tile_ids: number[] }
  | { type: "pass" }
  | { type: "resign" }

export type GameActionRequest = {
  expected_version: number
  turn_number: number
  idempotency_key: string
  action: GameMove
}

export type ApiErrorPayload = {
  schema_version: number
  code: string
  message: string
}

export type GameSession = {
  authority: GameAuthority
  gameId: string
  serverOrigin: string
}

export type ConnectionState = "connecting" | "live" | "reconnecting" | "offline"
