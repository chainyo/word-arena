import { describe, expect, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"

import { classifySessionFailure, connectionMessage } from "../src/api/session"
import { boardNarration, GameBoard } from "../src/components/game/game-board"
import {
  MoveHistory,
  type MoveRecord,
  moveSummary,
} from "../src/components/game/move-history"
import { resolveTheme } from "../src/components/theme-provider"

const move: MoveRecord = {
  player: "Seat one",
  turn: 4,
  word: "ETE",
  score: 8,
  detail: "move played",
  elapsed: "authoritative",
}

describe("theme and motion preferences", () => {
  test("resolves light, dark, and system themes without changing semantic tokens", () => {
    expect(resolveTheme("light", true)).toBe("light")
    expect(resolveTheme("dark", false)).toBe("dark")
    expect(resolveTheme("system", true)).toBe("dark")
    expect(resolveTheme("system", false)).toBe("light")
  })

  test("defines reduced-motion and coarse-pointer alternatives", async () => {
    const css = await Bun.file(`${import.meta.dir}/../src/index.css`).text()
    expect(css).toContain("@media (prefers-reduced-motion: reduce)")
    expect(css).toContain("@media (pointer: coarse)")
    expect(css).toContain("min-height: 44px")
    expect(css).toContain("--tile-shadow:")
    expect(css).toContain("--overlay:")
    const app = await Bun.file(`${import.meta.dir}/../src/App.tsx`).text()
    expect(app).toContain('href="#main-content"')
    expect(app).toContain('id="main-content"')
    expect(app).toContain("data-game-status")
  })
})

describe("session recovery semantics", () => {
  test("distinguishes credentials, conflicts, network loss, and server failure", () => {
    expect(classifySessionFailure({ status: 401 })).toBe("credential")
    expect(classifySessionFailure({ code: "version_conflict" })).toBe(
      "conflict"
    )
    expect(
      classifySessionFailure({ code: "deadline_not_reached", status: 409 })
    ).toBe("server")
    expect(classifySessionFailure(new TypeError("fetch failed"))).toBe(
      "network"
    )
    expect(classifySessionFailure(new Error("failed"))).toBe("server")
  })

  test("describes every connection state without implying stale data is live", () => {
    expect(connectionMessage("live")).toContain("Connected")
    expect(connectionMessage("connecting")).toContain("Connecting")
    expect(connectionMessage("reconnecting")).toContain("bounded backoff")
    expect(connectionMessage("offline")).toContain("stale")
  })
})

describe("board and move narration", () => {
  test("summarizes committed, staged, and latest move state", () => {
    expect(
      boardNarration(
        { "7-7": { letter: "E" } },
        { "7-8": { letter: "T", staged: true } },
        moveSummary(move)
      )
    ).toBe(
      "Board contains 1 committed tile. 1 tile is staged. Seat one, turn 4: ETE, move played, 8 points. Use arrow keys to move between interactive squares."
    )
  })

  test("renders a focusable scroll region, described board, and move summaries", () => {
    const board = renderToStaticMarkup(
      <GameBoard
        announcement={moveSummary(move)}
        stagedTiles={{ "7-8": { letter: "T", staged: true } }}
        tiles={{ "7-7": { letter: "E", value: 1 } }}
      />
    )
    expect(board).toContain(
      'aria-label="Scrollable 15 by 15 word game board region"'
    )
    expect(board).toContain('tabindex="0"')
    expect(board).toContain("min-w-[42rem]")
    expect(board).toContain("Board contains 1 committed tile")
    expect(board).toContain("aria-describedby=")

    const history = renderToStaticMarkup(<MoveHistory moves={[move]} />)
    expect(history).toContain(moveSummary(move))
    expect(renderToStaticMarkup(<MoveHistory moves={[]} />)).toContain(
      "No completed moves yet"
    )
  })
})
