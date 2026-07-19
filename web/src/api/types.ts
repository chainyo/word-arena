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
  seat?: Seat
  rack?: PhysicalTile[]
  racks?: [PhysicalTile[], PhysicalTile[]]
}

export type GameInvalidation = {
  schema_version: number
  game_id: string
  version: number
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
