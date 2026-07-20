import { type KeyboardEvent, useId } from "react"

import type { Premium, Seat } from "@/api/types"
import { cn } from "@/lib/utils"

const BOARD_SIZE = 15

export type BoardTile = {
  letter: string
  value?: number
  recent?: boolean
  staged?: boolean
  owner?: Seat
}

type GameBoardProps = {
  announcement?: string
  disabled?: boolean
  onSquareSelect?: (row: number, column: number) => void
  premiums?: Record<string, Premium>
  stagedTiles?: Record<string, BoardTile>
  tiles: Record<string, BoardTile>
}

export function boardNarration(
  tiles: Record<string, BoardTile>,
  stagedTiles: Record<string, BoardTile> = {},
  announcement?: string
): string {
  const occupied = Object.keys(tiles).length
  const staged = Object.keys(stagedTiles).length
  const parts = [
    `Board contains ${occupied} committed ${occupied === 1 ? "tile" : "tiles"}.`,
  ]
  if (staged > 0) {
    parts.push(`${staged} ${staged === 1 ? "tile is" : "tiles are"} staged.`)
  }
  if (announcement) parts.push(announcement)
  parts.push("Use arrow keys to move between interactive squares.")
  return parts.join(" ")
}

const premiumLabels: Partial<Record<Premium, string>> = {
  double_letter: "DL",
  double_word: "DW",
  triple_letter: "TL",
  triple_word: "TW",
}

const premiumNames: Partial<Record<Premium, string>> = {
  double_letter: "double letter",
  double_word: "double word",
  triple_letter: "triple letter",
  triple_word: "triple word",
}

const premiumClasses: Partial<Record<Premium, string>> = {
  double_letter: "bg-board-double-letter text-board-label",
  double_word: "bg-board-double-word text-board-label",
  triple_letter: "bg-board-triple-letter text-board-label",
  triple_word: "bg-board-triple-word text-board-label",
}

const squares = Array.from({ length: BOARD_SIZE * BOARD_SIZE }, (_, index) => ({
  row: Math.floor(index / BOARD_SIZE),
  column: index % BOARD_SIZE,
}))

const columnLabels = Array.from({ length: BOARD_SIZE }, (_, index) =>
  String.fromCharCode(65 + index)
)

function squareName(row: number, column: number) {
  return `${String.fromCharCode(65 + column)}${row + 1}`
}

export function boardFocusTarget(
  row: number,
  column: number,
  key: string
): { row: number; column: number } | undefined {
  const offsets: Record<string, [number, number]> = {
    ArrowDown: [1, 0],
    ArrowLeft: [0, -1],
    ArrowRight: [0, 1],
    ArrowUp: [-1, 0],
  }
  const offset = offsets[key]
  if (!offset) return undefined
  const target = { row: row + offset[0], column: column + offset[1] }
  if (
    target.row < 0 ||
    target.row >= BOARD_SIZE ||
    target.column < 0 ||
    target.column >= BOARD_SIZE
  ) {
    return undefined
  }
  return target
}

function moveBoardFocus(event: KeyboardEvent<HTMLButtonElement>) {
  const targetCoordinate = boardFocusTarget(
    Number(event.currentTarget.dataset.row),
    Number(event.currentTarget.dataset.column),
    event.key
  )
  if (!targetCoordinate) return
  event.preventDefault()
  const board = event.currentTarget.closest("[data-game-board]")
  const target = board?.querySelector<HTMLButtonElement>(
    `[data-row="${targetCoordinate.row}"][data-column="${targetCoordinate.column}"]`
  )
  target?.focus()
}

function TileFace({ tile }: { tile: BoardTile }) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        "absolute inset-[7%] grid place-items-center rounded-[18%] bg-tile font-heading text-[clamp(0.55rem,1.4vw,1.05rem)] font-semibold text-tile-foreground shadow-[inset_0_-2px_0_var(--tile-edge),0_1px_2px_var(--tile-shadow)]",
        tile.owner === "one" &&
          "bg-tile-seat-one ring-1 ring-seat-one/35 ring-inset",
        tile.owner === "two" &&
          "bg-tile-seat-two ring-1 ring-seat-two/35 ring-inset",
        tile.owner === "three" &&
          "bg-tile-seat-three ring-1 ring-seat-three/35 ring-inset",
        tile.owner === "four" &&
          "bg-tile-seat-four ring-1 ring-seat-four/35 ring-inset",
        tile.recent && "ring-2 ring-primary ring-inset",
        tile.staged && "opacity-80 ring-2 ring-primary ring-dashed ring-inset"
      )}
      data-seat-owner={tile.owner}
    >
      {tile.letter}
      {tile.value === undefined ? null : (
        <span className="absolute right-[9%] bottom-[4%] text-[clamp(0.28rem,0.5vw,0.45rem)] leading-none font-medium">
          {tile.value}
        </span>
      )}
    </span>
  )
}

export function GameBoard({
  announcement,
  disabled = false,
  onSquareSelect,
  premiums = {},
  stagedTiles = {},
  tiles,
}: GameBoardProps) {
  const descriptionId = useId()
  const interactive = onSquareSelect !== undefined
  const firstOpen = squares.find(
    ({ column, row }) => !tiles[`${row}-${column}`]
  )
  const initialKey = tiles["7-7"]
    ? `${firstOpen?.row}-${firstOpen?.column}`
    : "7-7"
  const narration = boardNarration(tiles, stagedTiles, announcement)
  return (
    <section
      aria-label="Scrollable 15 by 15 word game board region"
      className="w-full overflow-x-auto overscroll-x-contain pb-2"
      // biome-ignore lint/a11y/noNoninteractiveTabindex: keyboard users must be able to scroll the board at narrow widths and high zoom.
      tabIndex={0}
    >
      <p className="sr-only" id={descriptionId}>
        {narration}
      </p>
      <div className="mx-auto w-full min-w-[42rem] max-w-[720px]">
        <div className="mb-1.5 grid grid-cols-[repeat(15,minmax(0,1fr))] px-1.5 text-center font-mono text-[9px] text-muted-foreground sm:text-[10px]">
          {columnLabels.map((label) => (
            <span key={label}>{label}</span>
          ))}
        </div>
        <ol
          aria-describedby={descriptionId}
          aria-label="15 by 15 word game board"
          className="grid touch-manipulation grid-cols-[repeat(15,minmax(0,1fr))] gap-px rounded-xl bg-board-line p-1.5 shadow-inner ring-1 ring-foreground/10"
          data-game-board="true"
        >
          {squares.map(({ column, row }) => {
            const key = `${row}-${column}`
            const premium = premiums[key] ?? "normal"
            const tile = tiles[key]
            const staged = stagedTiles[key]
            const shownTile = tile ?? staged
            const name = squareName(row, column)
            const premiumName = premiumNames[premium]
            const description = shownTile
              ? `${name}: ${shownTile.letter}${shownTile.value === undefined ? "" : `, ${shownTile.value} points`}${shownTile.owner ? `, played by seat ${shownTile.owner}` : ""}${shownTile.recent ? ", part of the latest move" : ""}${shownTile.staged ? ", staged for this move" : ""}`
              : premiumName
                ? `${name}: ${premiumName} score`
                : `${name}: empty`
            const content = (
              <>
                {premium !== "normal" && !shownTile ? (
                  <span
                    aria-hidden="true"
                    className="absolute inset-0 grid place-items-center font-heading text-[clamp(0.3rem,0.65vw,0.55rem)] font-semibold tracking-tight"
                  >
                    {premiumLabels[premium]}
                  </span>
                ) : null}
                {shownTile ? <TileFace tile={shownTile} /> : null}
              </>
            )
            return (
              <li
                className={cn(
                  "relative aspect-square min-w-0 list-none overflow-hidden rounded-[3px] bg-board",
                  premiumClasses[premium]
                )}
                key={key}
              >
                {interactive ? (
                  <button
                    aria-disabled={disabled || tile !== undefined}
                    aria-label={description}
                    className="absolute inset-0 size-full rounded-[3px] outline-none focus-visible:z-10 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-inset disabled:cursor-not-allowed"
                    data-column={column}
                    data-row={row}
                    onClick={() => {
                      if (!disabled && !tile) onSquareSelect(row, column)
                    }}
                    onKeyDown={moveBoardFocus}
                    tabIndex={key === initialKey ? 0 : -1}
                    type="button"
                  >
                    {content}
                  </button>
                ) : (
                  <span aria-label={description} role="img">
                    {content}
                  </span>
                )}
              </li>
            )
          })}
        </ol>
        <div className="mt-2 flex flex-wrap items-center justify-center gap-x-4 gap-y-1 text-[10px] text-muted-foreground sm:text-xs">
          <span className="flex items-center gap-1.5">
            <span className="size-2 rounded-sm bg-board-double-letter" /> DL
          </span>
          <span className="flex items-center gap-1.5">
            <span className="size-2 rounded-sm bg-board-triple-letter" /> TL
          </span>
          <span className="flex items-center gap-1.5">
            <span className="size-2 rounded-sm bg-board-double-word" /> DW
          </span>
          <span className="flex items-center gap-1.5">
            <span className="size-2 rounded-sm bg-board-triple-word" /> TW
          </span>
        </div>
      </div>
    </section>
  )
}
