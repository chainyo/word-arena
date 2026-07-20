import { describe, expect, mock, test } from "bun:test"
import { render } from "@testing-library/react"
import userEvent from "@testing-library/user-event"

import { AgentConsole } from "../src/components/game/agent-console"
import { GameBoard } from "../src/components/game/game-board"
import { GameRack } from "../src/components/game/game-rack"

describe("browser-like game component interaction", () => {
  test("separates each seat and match lifecycle into activity tabs", async () => {
    const user = userEvent.setup()
    const { getByRole, queryByText } = render(
      <AgentConsole
        activity={{
          gameId: "game-tabs",
          events: [
            {
              sequence: 1,
              atUnixMs: 10_000,
              kind: "match_started",
              message: "Match started",
            },
            {
              sequence: 2,
              atUnixMs: 11_000,
              seat: "one",
              kind: "turn_started",
              message: "Seat one is thinking",
            },
            {
              sequence: 3,
              atUnixMs: 12_000,
              seat: "two",
              kind: "turn_failed",
              message: "Seat two failed",
            },
            {
              sequence: 4,
              atUnixMs: 12_500,
              seat: "one",
              kind: "diagnostic",
              message: "Seat one called the referee",
            },
          ],
        }}
        now={13_000}
        seatNames={["Codex", "Claude Code"]}
      />
    )

    expect(queryByText("Seat one is thinking")).not.toBeNull()
    const seatOneLog = getByRole("list", {
      name: "Seat one agent activity log",
    })
    expect(
      seatOneLog.textContent?.indexOf("Seat one is thinking")
    ).toBeLessThan(
      seatOneLog.textContent?.indexOf("Seat one called the referee") ?? -1
    )
    expect(queryByText("Seat two failed")).toBeNull()
    await user.click(getByRole("tab", { name: /Seat 2 Claude Code/ }))
    expect(queryByText("Seat one is thinking")).toBeNull()
    expect(queryByText("Seat two failed")).not.toBeNull()
    await user.click(getByRole("tab", { name: "Match" }))
    expect(queryByText("Match started")).not.toBeNull()
    expect(queryByText("Seat two failed")).toBeNull()
  })

  test("moves board focus with the keyboard and stages the focused square", async () => {
    const user = userEvent.setup()
    const onSquareSelect = mock(() => undefined)
    const { getByRole } = render(
      <GameBoard onSquareSelect={onSquareSelect} tiles={{}} />
    )

    const center = getByRole("button", { name: "H8: empty" })
    const right = getByRole("button", { name: "I8: empty" })
    center.focus()
    await user.keyboard("{ArrowRight}{Enter}")

    expect(document.activeElement).toBe(right)
    expect(onSquareSelect).toHaveBeenCalledWith(7, 8)
  })

  test("exposes rack selection and placed-tile restoration as buttons", async () => {
    const user = userEvent.setup()
    const onTileSelect = mock(() => undefined)
    const onPlacedTileSelect = mock(() => undefined)
    const { getByRole, rerender } = render(
      <GameRack
        label="Seat one rack"
        mode="place"
        onTileSelect={onTileSelect}
        tiles={[{ id: 100, letter: "E", value: 1 }]}
      />
    )

    await user.click(getByRole("button", { name: "E, 1 points" }))
    expect(onTileSelect).toHaveBeenCalledWith(100)

    rerender(
      <GameRack
        label="Seat one rack"
        mode="place"
        onPlacedTileSelect={onPlacedTileSelect}
        onTileSelect={onTileSelect}
        placedIds={[100]}
        tiles={[{ id: 100, letter: "E", value: 1 }]}
      />
    )
    await user.click(
      getByRole("button", {
        name: "E, 1 points, staged on board; activate to return to rack",
      })
    )
    expect(onPlacedTileSelect).toHaveBeenCalledWith(100)
  })
})
