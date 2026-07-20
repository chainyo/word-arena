import type {
  BoardTile,
  GameEvent,
  PublicReplayExport,
  ReplayBundle,
  Seat,
} from "@/api/types"

export type ReplayFrame = {
  sequence: number
  board: Array<BoardTile | null>
  scores: number[]
  currentPlayer: Seat
  rackCounts: number[]
  bagCount: number
  phase: "active" | "finished"
  event?: GameEvent
}

export type ReplayStatistics = {
  turns: number
  moveScore: number
  averageMoveScore?: number
  bingos: number
  passes: number
  exchanges: number
  uniqueWords: number
}

function object(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined
}

function playerNumbers(value: unknown): number[] | undefined {
  return Array.isArray(value) &&
    value.length >= 2 &&
    value.length <= 4 &&
    value.every((item) => typeof item === "number")
    ? (value as number[])
    : undefined
}

function seat(value: unknown): Seat | undefined {
  return value === "one" ||
    value === "two" ||
    value === "three" ||
    value === "four"
    ? value
    : undefined
}

function integer(value: unknown): number | undefined {
  return typeof value === "number" && Number.isSafeInteger(value)
    ? value
    : undefined
}

export function replayFrame(
  replay: ReplayBundle,
  throughSequence: number
): ReplayFrame {
  const frame: ReplayFrame = {
    sequence: -1,
    board: Array.from({ length: 225 }, () => null),
    scores: [0, 0],
    currentPlayer: "one",
    rackCounts: [0, 0],
    bagCount: 0,
    phase: "active",
  }
  for (const event of replay.events) {
    if (event.sequence > throughSequence) break
    const kind = event.kind
    if (kind.type === "created") {
      frame.rackCounts = playerNumbers(kind.rack_counts) ?? frame.rackCounts
      frame.bagCount = integer(kind.bag_count) ?? frame.bagCount
    } else if (kind.type === "move_played") {
      const placements = Array.isArray(kind.placements) ? kind.placements : []
      for (const placementValue of placements) {
        const placement = object(placementValue)
        const coordinate = object(placement?.coordinate)
        const tile = object(placement?.tile)
        const row = integer(coordinate?.row)
        const column = integer(coordinate?.column)
        const tileId = integer(placement?.tile_id)
        if (
          row !== undefined &&
          column !== undefined &&
          row >= 0 &&
          row < 15 &&
          column >= 0 &&
          column < 15 &&
          tileId !== undefined &&
          typeof tile?.letter === "string" &&
          typeof tile?.is_blank === "boolean"
        ) {
          frame.board[row * 15 + column] = {
            tile_id: tileId,
            letter: tile.letter,
            is_blank: tile.is_blank as boolean,
          }
        }
      }
      frame.scores = playerNumbers(kind.scores_after) ?? frame.scores
      frame.rackCounts =
        playerNumbers(kind.rack_counts_after) ?? frame.rackCounts
      frame.bagCount = integer(kind.bag_count_after) ?? frame.bagCount
      frame.currentPlayer = seat(kind.next_player) ?? frame.currentPlayer
      if (kind.result !== null && kind.result !== undefined) {
        frame.phase = "finished"
      }
    } else if (kind.type === "passed") {
      frame.currentPlayer = seat(kind.next_player) ?? frame.currentPlayer
      if (kind.result !== null && kind.result !== undefined) {
        frame.phase = "finished"
      }
    } else if (kind.type === "exchanged") {
      frame.rackCounts =
        playerNumbers(kind.rack_counts_after) ?? frame.rackCounts
      frame.bagCount = integer(kind.bag_count_after) ?? frame.bagCount
      frame.currentPlayer = seat(kind.next_player) ?? frame.currentPlayer
      if (kind.result !== null && kind.result !== undefined) {
        frame.phase = "finished"
      }
    } else if (kind.type === "resigned") {
      const result = object(kind.result)
      frame.scores = playerNumbers(result?.scores) ?? frame.scores
      frame.phase = "finished"
    }
    frame.sequence = event.sequence
    frame.event = event
  }
  return frame
}

export function replayStatistics(replay: ReplayBundle): ReplayStatistics {
  let turns = 0
  let moveScore = 0
  let bingos = 0
  let passes = 0
  let exchanges = 0
  const words = new Set<string>()
  for (const event of replay.events) {
    if (event.kind.type === "created") continue
    turns += 1
    if (event.kind.type === "move_played") {
      moveScore += integer(event.kind.score) ?? 0
      if ((integer(event.kind.bingo_bonus) ?? 0) > 0) bingos += 1
      const formed = Array.isArray(event.kind.words) ? event.kind.words : []
      for (const value of formed) {
        const word = object(value)?.text
        if (typeof word === "string") words.add(word)
      }
    } else if (event.kind.type === "passed") {
      passes += 1
    } else if (event.kind.type === "exchanged") {
      exchanges += 1
    }
  }
  const scoringTurns = replay.events.filter(
    (event) => event.kind.type === "move_played"
  ).length
  return {
    turns,
    moveScore,
    averageMoveScore: scoringTurns > 0 ? moveScore / scoringTurns : undefined,
    bingos,
    passes,
    exchanges,
    uniqueWords: words.size,
  }
}

export function formatStatistic(
  value: number | undefined,
  style: "integer" | "decimal" | "percent" = "integer"
): string {
  if (value === undefined || !Number.isFinite(value)) return "Not recorded"
  if (style === "percent") {
    return new Intl.NumberFormat("en", {
      style: "percent",
      maximumFractionDigits: 1,
    }).format(value)
  }
  return new Intl.NumberFormat("en", {
    maximumFractionDigits: style === "decimal" ? 1 : 0,
  }).format(value)
}

export function filterReplayEvents(
  events: GameEvent[],
  query: string,
  kind: string
): GameEvent[] {
  const normalized = query.trim().toLocaleUpperCase("en")
  return events.filter((event) => {
    if (kind !== "all" && event.kind.type !== kind) return false
    if (!normalized) return true
    return JSON.stringify(event.kind)
      .toLocaleUpperCase("en")
      .includes(normalized)
  })
}

export function publicReplayExport(replay: ReplayBundle): PublicReplayExport {
  return {
    schema_version: 1,
    kind: "word_arena_public_replay",
    replay: {
      schema_version: replay.schemaVersion,
      ruleset_identity: {
        schema_version: replay.rulesetIdentity.schemaVersion,
        ruleset_id: replay.rulesetIdentity.rulesetId,
        content_sha256: replay.rulesetIdentity.contentSha256,
      },
      ruleset: replay.rulesetWire,
      lexicon: {
        pack_id: replay.lexicon.packId,
        pack_version: replay.lexicon.packVersion,
        format_version: replay.lexicon.formatVersion,
        locale: replay.lexicon.locale,
        normalization: replay.lexicon.normalization,
        content_sha256: replay.lexicon.contentSha256,
      },
      rng_algorithm: replay.rngAlgorithm,
      seed_reveal: [...replay.seedReveal],
      events: replay.eventsWire,
    },
    redactions: ["private_events", "capabilities"],
  }
}

export function assertPublicExportPrivacy(value: unknown): void {
  const forbidden = new Set([
    "rack",
    "racks",
    "private_events",
    "capability",
    "capabilities",
    "public_capability",
    "spectator_capability",
    "snapshot",
  ])
  const visit = (node: unknown): void => {
    if (Array.isArray(node)) {
      node.forEach(visit)
      return
    }
    const record = object(node)
    if (!record) return
    for (const [key, child] of Object.entries(record)) {
      if (forbidden.has(key)) {
        throw new Error(`public export contains forbidden ${key} data`)
      }
      visit(child)
    }
  }
  visit(value)
}

export function serializePublicReplay(replay: ReplayBundle): string {
  const exported = publicReplayExport(replay)
  assertPublicExportPrivacy(exported)
  return `${JSON.stringify(exported, null, 2)}\n`
}

export function seedHex(seed: number[]): string {
  return seed.map((byte) => byte.toString(16).padStart(2, "0")).join("")
}
