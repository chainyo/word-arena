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
  type Ruleset,
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
  const letter = string(item.letter, `${label}.letter`)
  if (!/^[A-Z]$/.test(letter)) {
    throw new DecodeError(`${label}.letter must be one physical A-Z letter`)
  }
  return {
    tile_id: integer(item.tile_id, `${label}.tile_id`),
    letter,
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
  const token =
    kind === "letter" ? string(face.token, `${label}.face.token`) : undefined
  if (token !== undefined && !/^[A-Z]$/.test(token)) {
    throw new DecodeError(`${label}.face.token must be one physical A-Z letter`)
  }
  return {
    id: integer(item.id, `${label}.id`),
    face: kind === "blank" ? { kind } : { kind, token: token as string },
  }
}

function coordinate(value: unknown, label: string) {
  const item = record(value, label)
  return {
    row: integer(item.row, `${label}.row`),
    column: integer(item.column, `${label}.column`),
  }
}

export function decodeRuleset(value: unknown): Ruleset {
  const envelope = record(value, "rules envelope")
  if (integer(envelope.schema_version, "API schema") !== API_SCHEMA_VERSION) {
    throw new DecodeError("unsupported API schema")
  }
  const rules = record(envelope.data, "ruleset")
  const game = record(rules.game, "ruleset.game")
  const board = record(game.board, "ruleset.game.board")
  const width = integer(board.width, "ruleset.game.board.width")
  const height = integer(board.height, "ruleset.game.board.height")
  const squares = array(board.squares, "ruleset.game.board.squares").map(
    (value, index) => {
      const square = record(value, `ruleset square ${index}`)
      return {
        coordinate: coordinate(
          square.coordinate,
          `ruleset square ${index}.coordinate`
        ),
        premium: literal(
          square.premium,
          [
            "normal",
            "double_letter",
            "triple_letter",
            "double_word",
            "triple_word",
          ],
          `ruleset square ${index}.premium`
        ),
      }
    }
  )
  if (width !== 15 || height !== 15 || squares.length !== 225) {
    throw new DecodeError("ruleset board must be 15 by 15")
  }
  const tiles = array(game.tiles, "ruleset.game.tiles").map((value, index) => {
    const definition = record(value, `tile definition ${index}`)
    const decoded = physicalTile(
      { id: index, face: definition.face },
      `tile definition ${index}`
    )
    return {
      face: decoded.face,
      count: integer(definition.count, `tile definition ${index}.count`),
      value: integer(definition.value, `tile definition ${index}.value`),
    }
  })
  return {
    schema_version: integer(rules.schema_version, "ruleset.schema_version"),
    id: literal(rules.id, ["english-v1", "french-v1"], "ruleset.id"),
    language: literal(
      rules.language,
      ["english", "french"],
      "ruleset.language"
    ),
    game: {
      board: { width, height, squares },
      rack_capacity: integer(game.rack_capacity, "ruleset.game.rack_capacity"),
      bingo_bonus: integer(game.bingo_bonus, "ruleset.game.bingo_bonus"),
      exchange_minimum: integer(
        game.exchange_minimum,
        "ruleset.game.exchange_minimum"
      ),
      scoreless_turn_limit: integer(
        game.scoreless_turn_limit,
        "ruleset.game.scoreless_turn_limit"
      ),
      tiles,
    },
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
  const deadline =
    view.turn_deadline === null || view.turn_deadline === undefined
      ? undefined
      : record(view.turn_deadline, "game view.turn_deadline")
  const turnDeadline = deadline
    ? {
        turn: integer(deadline.turn, "turn deadline.turn"),
        seat: seat(deadline.seat, "turn deadline.seat"),
        deadlineAt: integer(deadline.deadline_at, "turn deadline.deadline_at"),
        policyVersion: integer(
          deadline.policy_version,
          "turn deadline.policy_version"
        ),
      }
    : undefined
  const game = record(view.game, "game view.game")

  if (authority === "public") {
    return {
      authority,
      observedAt,
      public: publicProjection(game),
      turnDeadline,
    }
  }

  if (authority === "seat") {
    return {
      authority,
      observedAt,
      turnDeadline,
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
    turnDeadline,
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
