import { describe, expect, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import { decodeReplayBundle } from "../src/api/decode"
import type { LexiconIdentity, ReplayBundle, Ruleset } from "../src/api/types"
import { ReplayView } from "../src/components/replay/replay-view"
import {
  assertPublicExportPrivacy,
  filterReplayEvents,
  formatStatistic,
  publicReplayExport,
  replayFrame,
  replayStatistics,
  serializePublicReplay,
} from "../src/replay"
import { routeAuthority } from "../src/routes"

const lexicon: LexiconIdentity = {
  packId: "word-arena-lexicon-en",
  packVersion: "1.0.0",
  formatVersion: 1,
  locale: "en",
  normalization: { identity: "physical-latin-v1" },
  contentSha256: "b".repeat(64),
}

const ruleset: Ruleset = {
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
        premium: index === 112 ? "double_word" : "normal",
      })),
    },
    rack_capacity: 7,
    bingo_bonus: 50,
    exchange_minimum: 7,
    scoreless_turn_limit: 6,
    tiles: [
      { face: { kind: "letter", token: "E" }, count: 12, value: 1 },
      { face: { kind: "blank" }, count: 2, value: 0 },
    ],
  },
}

const rulesetWire = {
  schema_version: 1,
  id: "english-v1",
  language: "english",
  lexicon: {
    pack_id: lexicon.packId,
    pack_version: lexicon.packVersion,
    format_version: lexicon.formatVersion,
    locale: lexicon.locale,
    normalization: lexicon.normalization,
    content_sha256: lexicon.contentSha256,
  },
  game: ruleset.game,
}

function replayFixture(): ReplayBundle {
  return {
    schemaVersion: 3,
    observedAt: 1_700_000_000_000,
    rulesetIdentity: {
      schemaVersion: 1,
      rulesetId: "english-v1",
      contentSha256: "a".repeat(64),
    },
    ruleset,
    rulesetWire,
    lexicon,
    rngAlgorithm: "xoshiro256-star-star-v1",
    seedReveal: Array.from({ length: 32 }, (_, index) => index),
    events: [
      {
        sequence: 0,
        kind: {
          type: "created",
          rack_counts: [7, 7],
          bag_count: 86,
        },
      },
      {
        sequence: 1,
        kind: {
          type: "move_played",
          player: "one",
          placements: [
            {
              tile_id: 8,
              coordinate: { row: 7, column: 7 },
              tile: { letter: "E", is_blank: false },
            },
          ],
          words: [{ text: "ETE" }],
          bingo_bonus: 0,
          score: 12,
          scores_after: [12, 0],
          rack_counts_after: [7, 7],
          bag_count_after: 85,
          next_player: "two",
          result: null,
        },
      },
      {
        sequence: 2,
        kind: {
          type: "passed",
          player: "two",
          next_player: "one",
          result: null,
        },
      },
      {
        sequence: 3,
        kind: {
          type: "resigned",
          player: "one",
          result: { scores: [12, 0] },
        },
      },
    ],
    eventsWire: [
      {
        sequence: 0,
        visibility: { scope: "public" },
        kind: { type: "created" },
      },
      {
        sequence: 1,
        visibility: { scope: "public" },
        kind: { type: "move_played" },
      },
      {
        sequence: 2,
        visibility: { scope: "public" },
        kind: { type: "passed" },
      },
      {
        sequence: 3,
        visibility: { scope: "public" },
        kind: { type: "resigned" },
      },
    ],
    privateEvents: [
      {
        sequence: 1,
        seat: "one",
        removed: [{ id: 8 }],
        drawn: [{ id: 9 }],
        rack_after: [{ id: 9 }],
      },
    ],
  }
}

describe("explicit route authority", () => {
  test("separates operator, player, spectator, replay, and aggregate routes", () => {
    expect(routeAuthority("/operator")).toBe("local_operator")
    expect(routeAuthority("/games/game-one/player")).toBe("competitive_seat")
    expect(routeAuthority("/games/game-one/spectator")).toBe("human_spectator")
    expect(routeAuthority("/games/game-one/replay")).toBe("human_spectator")
    expect(routeAuthority("/tournaments/cup/standings")).toBe("public_observer")
    expect(routeAuthority("/agents/codex")).toBe("public_observer")
    expect(routeAuthority("/unknown")).toBeUndefined()
  })
})

describe("recorded replay", () => {
  test("decodes the finished spectator artifact and preserves exact wire inputs", () => {
    const fixture = replayFixture()
    const decoded = decodeReplayBundle({
      schema_version: 1,
      data: {
        observed_at: fixture.observedAt,
        replay: {
          schema_version: fixture.schemaVersion,
          ruleset_identity: {
            schema_version: fixture.rulesetIdentity.schemaVersion,
            ruleset_id: fixture.rulesetIdentity.rulesetId,
            content_sha256: fixture.rulesetIdentity.contentSha256,
          },
          ruleset: fixture.rulesetWire,
          lexicon: rulesetWire.lexicon,
          rng_algorithm: fixture.rngAlgorithm,
          seed_reveal: fixture.seedReveal,
          events: fixture.eventsWire,
          private_events: fixture.privateEvents,
        },
      },
    })
    expect(decoded.rulesetWire).toEqual(rulesetWire)
    expect(decoded.eventsWire).toEqual(fixture.eventsWire)
    expect(decoded.seedReveal).toHaveLength(32)
  })

  test("steps public state without mutating the bundle", () => {
    const replay = replayFixture()
    expect(replayFrame(replay, 0).board[112]).toBeNull()
    const moved = replayFrame(replay, 1)
    expect(moved.board[112]).toEqual({
      tile_id: 8,
      letter: "E",
      is_blank: false,
    })
    expect(moved.scores).toEqual([12, 0])
    expect(moved.currentPlayer).toBe("two")
    expect(replayFrame(replay, 2).currentPlayer).toBe("one")
    expect(replayFrame(replay, 3).phase).toBe("finished")
    expect(replay.privateEvents[0]?.rack_after).toEqual([{ id: 9 }])
  })

  test("formats derived statistics and missing values consistently", () => {
    const statistics = replayStatistics(replayFixture())
    expect(statistics).toEqual({
      turns: 3,
      moveScore: 12,
      averageMoveScore: 12,
      bingos: 0,
      passes: 1,
      exchanges: 0,
      uniqueWords: 1,
    })
    expect(formatStatistic(statistics.averageMoveScore, "decimal")).toBe("12")
    expect(formatStatistic(0.534, "percent")).toBe("53.4%")
    expect(formatStatistic(undefined)).toBe("Not recorded")
    expect(
      filterReplayEvents(replayFixture().events, "ETE", "all")
    ).toHaveLength(1)
  })

  test("exports only the public reproducibility artifact", () => {
    const replay = replayFixture()
    const exported = publicReplayExport(replay)
    expect(() => assertPublicExportPrivacy(exported)).not.toThrow()
    expect(exported.replay.seed_reveal).toHaveLength(32)
    expect(exported.replay.ruleset_identity.content_sha256).toBe("a".repeat(64))
    expect(exported.replay.lexicon.pack_version).toBe("1.0.0")
    const json = serializePublicReplay(replay)
    expect(json).not.toContain('"private_events":')
    expect(json).not.toContain('"rack_after":')
    expect(json).not.toContain('"capability":')
    expect(json).toContain('"seed_reveal"')
  })

  test("renders controls, exact identities, filters, and public export action", () => {
    const html = renderToStaticMarkup(
      <ReplayView gameId="game-one" replay={replayFixture()} />
    )
    expect(html).toContain("Post-game board")
    expect(html).toContain("First replay event")
    expect(html).toContain("Search replay events")
    expect(html).toContain("Exact replay inputs")
    expect(html).toContain("Export public replay")
    expect(html).toContain("Public export always removes them")
  })
})
