import { describe, expect, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"

import type { PhysicalTile } from "../src/api/types"
import { AgentConsole } from "../src/components/game/agent-console"
import {
  displayLetterValues,
  displayPremiums,
} from "../src/components/game/display-rules"
import { boardFocusTarget, GameBoard } from "../src/components/game/game-board"
import { formatClock, GameClock } from "../src/components/game/game-clock"
import { GameRack } from "../src/components/game/game-rack"
import {
  EMPTY_MOVE_DRAFT,
  physicalLetter,
  removePlacement,
  selectRackTile,
  setDraftMode,
  stageSelectedTile,
} from "../src/components/game/move-draft"
import { tileOwnersFromEvents } from "../src/components/game/move-ownership"

const rack: PhysicalTile[] = [
  { id: 1, face: { kind: "letter", token: "E" } },
  { id: 2, face: { kind: "blank" } },
]

describe("pointer and keyboard move drafting", () => {
  test("selects and stages a physical letter without mutating the source rack", () => {
    const selected = selectRackTile(EMPTY_MOVE_DRAFT, 1)
    const result = stageSelectedTile(selected, rack, { row: 7, column: 7 })
    expect(result.needsBlank).toBe(false)
    expect(result.draft.placements).toEqual([
      {
        tile_id: 1,
        coordinate: { row: 7, column: 7 },
        tile: { letter: "E", is_blank: false },
      },
    ])
    expect(rack).toHaveLength(2)
    expect(result.draft.selectedTileId).toBeUndefined()
  })

  test("requires and records an A-Z assignment for blanks", () => {
    const selected = selectRackTile(EMPTY_MOVE_DRAFT, 2)
    expect(
      stageSelectedTile(selected, rack, { row: 7, column: 8 }).needsBlank
    ).toBe(true)
    const assigned = stageSelectedTile(
      selected,
      rack,
      { row: 7, column: 8 },
      "É"
    )
    expect(assigned.draft.placements).toHaveLength(0)
    const valid = stageSelectedTile(selected, rack, { row: 7, column: 8 }, "E")
    expect(valid.draft.placements[0]?.tile).toEqual({
      letter: "E",
      is_blank: true,
    })
  })

  test("keeps placement and exchange selections mutually exclusive", () => {
    const staged = stageSelectedTile(
      selectRackTile(EMPTY_MOVE_DRAFT, 1),
      rack,
      { row: 7, column: 7 }
    ).draft
    const exchange = setDraftMode(staged, "exchange")
    expect(exchange.placements).toHaveLength(0)
    const selected = selectRackTile(exchange, 2)
    expect(selected.exchangeIds).toEqual([2])
    expect(selectRackTile(selected, 2).exchangeIds).toHaveLength(0)
    expect(removePlacement(staged, 1).placements).toHaveLength(0)
  })

  test("moves board focus with arrow keys without a pointer", () => {
    expect(boardFocusTarget(7, 7, "ArrowRight")).toEqual({ row: 7, column: 8 })
    expect(boardFocusTarget(7, 7, "ArrowUp")).toEqual({ row: 6, column: 7 })
    expect(boardFocusTarget(0, 0, "ArrowLeft")).toBeUndefined()
    expect(boardFocusTarget(14, 14, "ArrowDown")).toBeUndefined()
  })
})

describe("English and French physical display rules", () => {
  test("uses the language-specific immutable tile values", () => {
    const english = displayLetterValues("english-v1")
    const french = displayLetterValues("french-v1")
    expect(english.get("K")).toBe(5)
    expect(french.get("K")).toBe(10)
    expect(english.get("W")).toBe(4)
    expect(french.get("W")).toBe(10)
    expect(english.get("?")).toBe(0)
    expect(french.get("?")).toBe(0)
    expect(physicalLetter(rack[1] as PhysicalTile)).toBe("?")
  })

  test("retains the shared premium coordinates", () => {
    const premiums = displayPremiums()
    expect(premiums["7-7"]).toBe("double_word")
    expect(premiums["0-0"]).toBe("triple_word")
    expect(premiums["1-5"]).toBe("triple_letter")
    expect(premiums["0-3"]).toBe("double_letter")
  })
})

describe("semantic game components", () => {
  test("renders redacted agent activity with a live elapsed turn", () => {
    const html = renderToStaticMarkup(
      <AgentConsole
        activity={{
          gameId: "game-one",
          events: [
            {
              sequence: 1,
              atUnixMs: 10_000,
              seat: "one",
              kind: "turn_started",
              message: "Turn 3 started",
              turnId: "1-3",
            },
          ],
        }}
        now={15_000}
      />
    )
    expect(html).toContain("Agent activity")
    expect(html).toContain("Turn 3 started")
    expect(html).toContain("5s live")
    expect(html).toContain("Seat one")
  })

  test("renders coordinates, premium meaning, staged state, and roving board focus", () => {
    const html = renderToStaticMarkup(
      <GameBoard
        onSquareSelect={() => undefined}
        premiums={displayPremiums()}
        stagedTiles={{ "7-7": { letter: "E", staged: true, value: 1 } }}
        tiles={{}}
      />
    )
    expect(html).toContain('aria-label="15 by 15 word game board"')
    expect(html).toContain("H8: E, 1 points, staged for this move")
    expect(html).toContain('data-row="7"')
    expect(html).toContain('data-column="7"')
    expect(html).toContain('tabindex="0"')
    expect(html).toContain("triple word score")
  })

  test("reconstructs and narrates committed tile ownership", () => {
    const owners = tileOwnersFromEvents([
      {
        sequence: 1,
        kind: {
          type: "move_played",
          player: "one",
          placements: [{ tile_id: 101 }],
        },
      },
      {
        sequence: 2,
        kind: {
          type: "move_played",
          player: "two",
          placements: [{ tile_id: 202 }],
        },
      },
    ])
    const html = renderToStaticMarkup(
      <GameBoard
        tiles={{
          "7-7": { letter: "E", owner: owners.get(101), value: 1 },
          "7-8": { letter: "T", owner: owners.get(202), value: 1 },
        }}
      />
    )

    expect(owners.get(101)).toBe("one")
    expect(owners.get(202)).toBe("two")
    expect(html).toContain('data-seat-owner="one"')
    expect(html).toContain('data-seat-owner="two"')
    expect(html).toContain("H8: E, 1 points, played by seat one")
    expect(html).toContain("I8: T, 1 points, played by seat two")
  })

  test("renders rack selection and clock states accessibly", () => {
    const rackHtml = renderToStaticMarkup(
      <GameRack
        label="Seat one rack"
        mode="place"
        onTileSelect={() => undefined}
        selectedTileId={1}
        tiles={[
          { id: 1, letter: "E", value: 1 },
          { id: 2, letter: "?", value: 0 },
        ]}
      />
    )
    expect(rackHtml).toContain("E, 1 points, selected")
    expect(rackHtml).toContain("blank, 0 points")
    expect(rackHtml).toContain('aria-pressed="true"')

    const clockHtml = renderToStaticMarkup(
      <GameClock
        active
        deadlineAt={62_000}
        label="Seat one"
        observedAt={1_000}
      />
    )
    expect(clockHtml).toContain("Seat one clock: 01:01")
    expect(formatClock()).toBe("--:--")
  })
})
