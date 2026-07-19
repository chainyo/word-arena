import { describe, expect, mock, test } from "bun:test"
import { render } from "@testing-library/react"
import userEvent from "@testing-library/user-event"

import { GameBoard } from "../src/components/game/game-board"
import { GameRack } from "../src/components/game/game-rack"

describe("browser-like game component interaction", () => {
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
