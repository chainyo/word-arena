import {
  type AgentCatalogEntry,
  type AgentHarnessId,
  type AgentMatchActivity,
  type AgentMatchList,
  type AgentMatchRecovery,
  type AgentMatchStatus,
  type AgentSeatSelection,
  type AgentSeatStatus,
  API_SCHEMA_VERSION,
  type ApiErrorPayload,
  type BoardTile,
  type CreatedAgentMatch,
  type CreatedGame,
  type GameAuthority,
  type GameEvent,
  type GameInvalidation,
  type GameView,
  type LexiconIdentity,
  type PhysicalTile,
  PROJECTION_SCHEMA_VERSION,
  type PublicGameState,
  type PublicProjection,
  REPLAY_SCHEMA_VERSION,
  type ReplayBundle,
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

function boolean(value: unknown, label: string): boolean {
  if (typeof value !== "boolean") {
    throw new DecodeError(`${label} must be boolean`)
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

function playerValues(value: unknown, label: string): number[] {
  const values = array(value, label)
  if (values.length < 2 || values.length > 4) {
    throw new DecodeError(`${label} must contain between two and four values`)
  }
  return values.map((item, index) => integer(item, `${label}[${index}]`))
}

function seat(value: unknown, label: string): Seat {
  return literal(value, ["one", "two", "three", "four"], label)
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

function lexiconIdentity(value: unknown, label: string): LexiconIdentity {
  const lexicon = record(value, label)
  return {
    packId: string(lexicon.pack_id, `${label}.pack_id`),
    packVersion: string(lexicon.pack_version, `${label}.pack_version`),
    formatVersion: integer(lexicon.format_version, `${label}.format_version`),
    locale: string(lexicon.locale, `${label}.locale`),
    normalization: record(lexicon.normalization, `${label}.normalization`),
    contentSha256: string(lexicon.content_sha256, `${label}.content_sha256`),
  }
}

function rulesetData(value: unknown): Ruleset {
  const rules = record(value, "ruleset")
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
    lexicon: lexiconIdentity(rules.lexicon, "ruleset.lexicon"),
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

export function decodeRuleset(value: unknown): Ruleset {
  const envelope = record(value, "rules envelope")
  if (integer(envelope.schema_version, "API schema") !== API_SCHEMA_VERSION) {
    throw new DecodeError("unsupported API schema")
  }
  return rulesetData(envelope.data)
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
    scores: playerValues(state.scores, "public state.scores"),
    current_player: seat(state.current_player, "public state.current_player"),
    version: integer(state.version, "public state.version"),
    scoreless_turns: integer(
      state.scoreless_turns,
      "public state.scoreless_turns"
    ),
    rack_counts: playerValues(state.rack_counts, "public state.rack_counts"),
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
  if (racks.length < 2 || racks.length > 4) {
    throw new DecodeError(
      "spectator projection must contain between two and four racks"
    )
  }
  return {
    authority,
    observedAt,
    turnDeadline,
    public: publicProjection(game.public),
    racks: racks.map((rack, rackIndex) =>
      array(rack, `spectator rack ${rackIndex + 1}`).map((tile, tileIndex) =>
        physicalTile(tile, `spectator rack ${rackIndex + 1}[${tileIndex}]`)
      )
    ),
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

export function decodeCreatedGame(value: unknown): CreatedGame {
  const envelope = record(value, "create game envelope")
  if (integer(envelope.schema_version, "API schema") !== API_SCHEMA_VERSION) {
    throw new DecodeError("unsupported API schema")
  }
  const data = record(envelope.data, "created game")
  assertPrivacy(data.public, "public")
  return {
    gameId: string(data.game_id, "created game.game_id"),
    public: publicProjection(data.public),
    publicCapability: string(
      data.public_capability,
      "created game.public_capability"
    ),
    spectatorCapability: string(
      data.spectator_capability,
      "created game.spectator_capability"
    ),
  }
}

const AGENT_HARNESSES = [
  "codex",
  "claude_code",
  "cline",
  "pi",
] as const satisfies readonly AgentHarnessId[]

function agentSeatSelection(value: unknown, label: string): AgentSeatSelection {
  const selection = record(value, label)
  const kind = literal(selection.kind, ["agent", "human"], `${label}.kind`)
  if (kind === "human") {
    return { kind, name: string(selection.name, `${label}.name`) }
  }
  const model = selection.model
  return {
    kind,
    harness: literal(selection.harness, AGENT_HARNESSES, `${label}.harness`),
    model:
      model === undefined || model === null
        ? undefined
        : string(model, `${label}.model`),
  }
}

function agentSeatStatus(value: unknown, label: string): AgentSeatStatus {
  const item = record(value, label)
  const status = record(item.status, `${label}.status`)
  const state = literal(
    status.state,
    [
      "queued",
      "starting",
      "ready",
      "thinking",
      "waiting_for_human",
      "finished",
      "failed",
    ],
    `${label}.status.state`
  )
  return {
    seat: seat(item.seat, `${label}.seat`),
    participant: agentSeatSelection(item.participant, `${label}.participant`),
    state,
    failureCode:
      state === "failed"
        ? string(status.code, `${label}.status.code`)
        : undefined,
  }
}

function agentMatchStatusData(value: unknown): AgentMatchStatus {
  const item = record(value, "agent match status")
  if (
    integer(item.schema_version, "agent match status.schema_version") !==
    API_SCHEMA_VERSION
  ) {
    throw new DecodeError("unsupported agent match schema")
  }
  const seats = array(item.seats, "agent match status.seats")
  if (seats.length < 2 || seats.length > 4) {
    throw new DecodeError("agent match must contain between two and four seats")
  }
  return {
    gameId: string(item.game_id, "agent match status.game_id"),
    language: literal(
      item.language,
      ["english", "french"],
      "agent match status.language"
    ),
    mode: literal(
      item.mode,
      ["competitive", "practice"],
      "agent match status.mode"
    ),
    phase: literal(
      item.phase,
      ["active", "finished"],
      "agent match status.phase"
    ),
    orchestration: literal(
      item.orchestration,
      ["active", "finished", "interrupted"],
      "agent match status.orchestration"
    ),
    version: integer(item.version, "agent match status.version"),
    currentSeat: seat(item.current_seat, "agent match status.current_seat"),
    scores: playerValues(item.scores, "agent match status.scores"),
    createdAtUnixMs: integer(
      item.created_at_unix_ms,
      "agent match status.created_at_unix_ms"
    ),
    updatedAtUnixMs: integer(
      item.updated_at_unix_ms,
      "agent match status.updated_at_unix_ms"
    ),
    seats: seats.map((seat, index) =>
      agentSeatStatus(seat, `agent match status.seats[${index}]`)
    ),
  }
}

export function decodeAgentMatchList(value: unknown): AgentMatchList {
  const envelope = record(value, "agent match list envelope")
  if (integer(envelope.schema_version, "API schema") !== API_SCHEMA_VERSION) {
    throw new DecodeError("unsupported API schema")
  }
  const data = record(envelope.data, "agent match list")
  return {
    matches: array(data.matches, "agent match list.matches").map(
      agentMatchStatusData
    ),
  }
}

export function decodeAgentMatchRecovery(value: unknown): AgentMatchRecovery {
  const envelope = record(value, "agent match recovery envelope")
  if (integer(envelope.schema_version, "API schema") !== API_SCHEMA_VERSION) {
    throw new DecodeError("unsupported API schema")
  }
  const data = record(envelope.data, "agent match recovery")
  return {
    gameId: string(data.game_id, "agent match recovery.game_id"),
    spectatorCapability: string(
      data.spectator_capability,
      "agent match recovery.spectator_capability"
    ),
  }
}

export function decodeAgentCatalog(value: unknown): AgentCatalogEntry[] {
  const envelope = record(value, "agent catalog envelope")
  if (integer(envelope.schema_version, "API schema") !== API_SCHEMA_VERSION) {
    throw new DecodeError("unsupported API schema")
  }
  return array(envelope.data, "agent catalog").map((value, index) => {
    const item = record(value, `agent catalog[${index}]`)
    const version = item.version
    return {
      id: literal(item.id, AGENT_HARNESSES, `agent catalog[${index}].id`),
      displayName: string(
        item.display_name,
        `agent catalog[${index}].display_name`
      ),
      logo: string(item.logo, `agent catalog[${index}].logo`),
      available: boolean(item.available, `agent catalog[${index}].available`),
      compatible: boolean(
        item.compatible,
        `agent catalog[${index}].compatible`
      ),
      version:
        version === undefined || version === null
          ? undefined
          : string(version, `agent catalog[${index}].version`),
      minimumVersion: string(
        item.minimum_version,
        `agent catalog[${index}].minimum_version`
      ),
      diagnostic: string(item.diagnostic, `agent catalog[${index}].diagnostic`),
    }
  })
}

export function decodeAgentMatchStatus(value: unknown): AgentMatchStatus {
  const envelope = record(value, "agent match envelope")
  if (integer(envelope.schema_version, "API schema") !== API_SCHEMA_VERSION) {
    throw new DecodeError("unsupported API schema")
  }
  return agentMatchStatusData(envelope.data)
}

const AGENT_ACTIVITY_KINDS = [
  "match_started",
  "agent_starting",
  "agent_ready",
  "agent_failed",
  "turn_started",
  "tool_called",
  "diagnostic",
  "turn_completed",
  "turn_failed",
  "agent_finished",
  "match_finished",
] as const

export function decodeAgentMatchActivity(value: unknown): AgentMatchActivity {
  const envelope = record(value, "agent match activity envelope")
  if (integer(envelope.schema_version, "API schema") !== API_SCHEMA_VERSION) {
    throw new DecodeError("unsupported API schema")
  }
  const data = record(envelope.data, "agent match activity")
  if (
    integer(data.schema_version, "agent match activity.schema_version") !==
    API_SCHEMA_VERSION
  ) {
    throw new DecodeError("unsupported agent activity schema")
  }
  return {
    gameId: string(data.game_id, "agent match activity.game_id"),
    events: array(data.events, "agent match activity.events").map(
      (value, index) => {
        const event = record(value, `agent activity event[${index}]`)
        const seatValue = event.seat
        const turnId = event.turn_id
        const durationMs = event.duration_ms
        return {
          sequence: integer(
            event.sequence,
            `agent activity event[${index}].sequence`
          ),
          atUnixMs: integer(
            event.at_unix_ms,
            `agent activity event[${index}].at_unix_ms`
          ),
          seat:
            seatValue === undefined || seatValue === null
              ? undefined
              : seat(seatValue, `agent activity event[${index}].seat`),
          kind: literal(
            event.kind,
            AGENT_ACTIVITY_KINDS,
            `agent activity event[${index}].kind`
          ),
          message: string(
            event.message,
            `agent activity event[${index}].message`
          ),
          turnId:
            turnId === undefined || turnId === null
              ? undefined
              : string(turnId, `agent activity event[${index}].turn_id`),
          durationMs:
            durationMs === undefined || durationMs === null
              ? undefined
              : integer(
                  durationMs,
                  `agent activity event[${index}].duration_ms`
                ),
        }
      }
    ),
  }
}

export function decodeCreatedAgentMatch(value: unknown): CreatedAgentMatch {
  const envelope = record(value, "create agent match envelope")
  if (integer(envelope.schema_version, "API schema") !== API_SCHEMA_VERSION) {
    throw new DecodeError("unsupported API schema")
  }
  const data = record(envelope.data, "created agent match")
  assertPrivacy(data.public, "public")
  const humanCapability = data.human_capability
  return {
    gameId: string(data.game_id, "created agent match.game_id"),
    public: publicProjection(data.public),
    publicCapability: string(
      data.public_capability,
      "created agent match.public_capability"
    ),
    spectatorCapability: string(
      data.spectator_capability,
      "created agent match.spectator_capability"
    ),
    humanCapability:
      humanCapability === undefined || humanCapability === null
        ? undefined
        : string(humanCapability, "created agent match.human_capability"),
    status: agentMatchStatusData(data.status),
  }
}

export function decodeReplayBundle(value: unknown): ReplayBundle {
  const envelope = record(value, "replay envelope")
  if (integer(envelope.schema_version, "API schema") !== API_SCHEMA_VERSION) {
    throw new DecodeError("unsupported API schema")
  }
  const data = record(envelope.data, "replay view")
  const replay = record(data.replay, "replay")
  const replaySchema = integer(replay.schema_version, "replay.schema_version")
  if (replaySchema !== REPLAY_SCHEMA_VERSION) {
    throw new DecodeError("unsupported replay schema")
  }
  const identity = record(replay.ruleset_identity, "replay.ruleset_identity")
  const lexicon = lexiconIdentity(replay.lexicon, "replay.lexicon")
  const seedReveal = array(replay.seed_reveal, "replay.seed_reveal").map(
    (byte, index) => {
      const value = integer(byte, `replay.seed_reveal[${index}]`)
      if (value < 0 || value > 255) {
        throw new DecodeError("replay seed bytes must be between 0 and 255")
      }
      return value
    }
  )
  if (seedReveal.length !== 32) {
    throw new DecodeError("replay seed reveal must contain 32 bytes")
  }
  const events = array(replay.events, "replay.events").map((event, index) =>
    gameEvent(event, `replay.events[${index}]`)
  )
  const eventsWire = array(replay.events, "replay.events").map((event, index) =>
    record(event, `replay.events[${index}]`)
  )
  events.forEach((event, index) => {
    if (event.sequence !== index) {
      throw new DecodeError("replay event sequence must be contiguous")
    }
  })
  return {
    schemaVersion: replaySchema,
    observedAt: integer(data.observed_at, "replay view.observed_at"),
    rulesetIdentity: {
      schemaVersion: integer(
        identity.schema_version,
        "replay.ruleset_identity.schema_version"
      ),
      rulesetId: literal(
        identity.ruleset_id,
        ["english-v1", "french-v1"],
        "replay.ruleset_identity.ruleset_id"
      ),
      contentSha256: string(
        identity.content_sha256,
        "replay.ruleset_identity.content_sha256"
      ),
    },
    ruleset: rulesetData(replay.ruleset),
    rulesetWire: record(replay.ruleset, "replay.ruleset"),
    lexicon,
    rngAlgorithm: literal(
      replay.rng_algorithm,
      ["xoshiro256-star-star-v1"],
      "replay.rng_algorithm"
    ),
    seedReveal,
    events,
    eventsWire,
    privateEvents: array(replay.private_events, "replay.private_events").map(
      (event, index) => record(event, `replay.private_events[${index}]`)
    ),
  }
}
