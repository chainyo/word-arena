import {
  API_SCHEMA_VERSION,
  type ApiErrorPayload,
  type BoardTile,
  type GameAuthority,
  type GameEvent,
  type GameInvalidation,
  type GameView,
  type PhysicalTile,
  PROJECTION_SCHEMA_VERSION,
  type PublicGameState,
  type PublicProjection,
  type Seat,
} from "@/api/types"

export class DecodeError extends Error {
  constructor(message: string) {
    super(message)
    this.name = "DecodeError"
  }
}

function record(value: unknown, label: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new DecodeError(`${label} must be an object`)
  }
  return value as Record<string, unknown>
}

function string(value: unknown, label: string): string {
  if (typeof value !== "string" || value.length === 0) {
    throw new DecodeError(`${label} must be a non-empty string`)
  }
  return value
}

function integer(value: unknown, label: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value)) {
    throw new DecodeError(`${label} must be an integer`)
  }
  return value
}

function literal<T extends string>(
  value: unknown,
  allowed: readonly T[],
  label: string
): T {
  if (typeof value !== "string" || !allowed.includes(value as T)) {
    throw new DecodeError(`${label} is unsupported`)
  }
  return value as T
}

function array(value: unknown, label: string): unknown[] {
  if (!Array.isArray(value)) {
    throw new DecodeError(`${label} must be an array`)
  }
  return value
}

function pair(value: unknown, label: string): [number, number] {
  const values = array(value, label)
  if (values.length !== 2) {
    throw new DecodeError(`${label} must contain two values`)
  }
  return [integer(values[0], `${label}[0]`), integer(values[1], `${label}[1]`)]
}

function seat(value: unknown, label: string): Seat {
  return literal(value, ["one", "two"], label)
}

function boardTile(value: unknown, label: string): BoardTile {
  const item = record(value, label)
  return {
    tile_id: integer(item.tile_id, `${label}.tile_id`),
    letter: string(item.letter, `${label}.letter`),
    is_blank:
      typeof item.is_blank === "boolean"
        ? item.is_blank
        : (() => {
            throw new DecodeError(`${label}.is_blank must be boolean`)
          })(),
  }
}

function physicalTile(value: unknown, label: string): PhysicalTile {
  const item = record(value, label)
  const face = record(item.face, `${label}.face`)
  const kind = literal(face.kind, ["blank", "letter"], `${label}.face.kind`)
  return {
    id: integer(item.id, `${label}.id`),
    face:
      kind === "blank"
        ? { kind }
        : { kind, token: string(face.token, `${label}.face.token`) },
  }
}

function gameEvent(value: unknown, label: string): GameEvent {
  const event = record(value, label)
  const kind = record(event.kind, `${label}.kind`)
  string(kind.type, `${label}.kind.type`)
  return {
    sequence: integer(event.sequence, `${label}.sequence`),
    kind: kind as GameEvent["kind"],
  }
}

function publicState(value: unknown): PublicGameState {
  const state = record(value, "public state")
  const board = array(state.board, "public state.board")
  if (board.length !== 225) {
    throw new DecodeError("public state.board must contain 225 squares")
  }
  return {
    game_id: string(state.game_id, "public state.game_id"),
    ruleset_id: literal(
      state.ruleset_id,
      ["english-v1", "french-v1"],
      "public state.ruleset_id"
    ),
    mode: literal(
      state.mode ?? "competitive",
      ["competitive", "practice"],
      "public state.mode"
    ),
    board: board.map((tile, index) =>
      tile === null ? null : boardTile(tile, `public state.board[${index}]`)
    ),
    scores: pair(state.scores, "public state.scores"),
    current_player: seat(state.current_player, "public state.current_player"),
    version: integer(state.version, "public state.version"),
    scoreless_turns: integer(
      state.scoreless_turns,
      "public state.scoreless_turns"
    ),
    rack_counts: pair(state.rack_counts, "public state.rack_counts"),
    bag_count: integer(state.bag_count, "public state.bag_count"),
    phase: literal(state.phase, ["active", "finished"], "public state.phase"),
  }
}

function publicProjection(value: unknown): PublicProjection {
  const projection = record(value, "public projection")
  const schemaVersion = integer(
    projection.schema_version,
    "public projection.schema_version"
  )
  if (schemaVersion !== PROJECTION_SCHEMA_VERSION) {
    throw new DecodeError("unsupported public projection schema")
  }
  return {
    schema_version: schemaVersion,
    state: publicState(projection.state),
    events: array(projection.events, "public projection.events").map(
      (event, index) => gameEvent(event, `public projection.events[${index}]`)
    ),
  }
}

function assertPrivacy(value: unknown, authority: GameAuthority): void {
  const forbidden =
    authority === "public"
      ? new Set(["rack", "racks", "private_events", "bag", "seed", "snapshot"])
      : authority === "seat"
        ? new Set(["racks", "bag", "seed", "snapshot"])
        : new Set(["bag", "seed", "snapshot"])
  const visit = (node: unknown): void => {
    if (Array.isArray(node)) {
      node.forEach(visit)
      return
    }
    if (typeof node !== "object" || node === null) return
    for (const [key, child] of Object.entries(node)) {
      if (forbidden.has(key)) {
        throw new DecodeError(
          `forbidden ${key} field in ${authority} projection`
        )
      }
      visit(child)
    }
  }
  visit(value)
}

export function decodeGameView(
  value: unknown,
  authority: GameAuthority
): GameView {
  assertPrivacy(value, authority)
  const envelope = record(value, "API envelope")
  if (integer(envelope.schema_version, "API schema") !== API_SCHEMA_VERSION) {
    throw new DecodeError("unsupported API schema")
  }
  const view = record(envelope.data, "game view")
  const observedAt = integer(view.observed_at, "game view.observed_at")
  const game = record(view.game, "game view.game")

  if (authority === "public") {
    return { authority, observedAt, public: publicProjection(game) }
  }

  if (authority === "seat") {
    return {
      authority,
      observedAt,
      public: publicProjection(game.public),
      seat: seat(game.seat, "seat projection.seat"),
      rack: array(game.rack, "seat projection.rack").map((tile, index) =>
        physicalTile(tile, `seat projection.rack[${index}]`)
      ),
    }
  }

  const racks = array(game.racks, "spectator projection.racks")
  if (racks.length !== 2) {
    throw new DecodeError("spectator projection must contain two racks")
  }
  return {
    authority,
    observedAt,
    public: publicProjection(game.public),
    racks: [
      array(racks[0], "spectator rack one").map((tile, index) =>
        physicalTile(tile, `spectator rack one[${index}]`)
      ),
      array(racks[1], "spectator rack two").map((tile, index) =>
        physicalTile(tile, `spectator rack two[${index}]`)
      ),
    ],
  }
}

export function decodeInvalidation(value: unknown): GameInvalidation {
  const marker = record(value, "game invalidation")
  const schemaVersion = integer(marker.schema_version, "invalidation schema")
  if (schemaVersion !== API_SCHEMA_VERSION) {
    throw new DecodeError("unsupported invalidation schema")
  }
  return {
    schema_version: schemaVersion,
    game_id: string(marker.game_id, "invalidation game_id"),
    version: integer(marker.version, "invalidation version"),
  }
}

export function decodeApiError(value: unknown): ApiErrorPayload | undefined {
  try {
    const error = record(value, "API error")
    const schemaVersion = integer(error.schema_version, "API error schema")
    if (schemaVersion !== API_SCHEMA_VERSION) return undefined
    return {
      schema_version: schemaVersion,
      code: string(error.code, "API error code"),
      message: string(error.message, "API error message"),
    }
  } catch {
    return undefined
  }
}
