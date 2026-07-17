import { cn } from "@/lib/utils"

const BOARD_SIZE = 15

type Premium = "double-letter" | "double-word" | "triple-letter" | "triple-word"

export type BoardTile = {
  letter: string
  value: number
  recent?: boolean
}

type GameBoardProps = {
  tiles: Record<string, BoardTile>
}

const premiumLabels: Record<Premium, string> = {
  "double-letter": "DL",
  "double-word": "DW",
  "triple-letter": "TL",
  "triple-word": "TW",
}

const premiumNames: Record<Premium, string> = {
  "double-letter": "double letter",
  "double-word": "double word",
  "triple-letter": "triple letter",
  "triple-word": "triple word",
}

const premiumClasses: Record<Premium, string> = {
  "double-letter": "bg-board-double-letter text-board-label",
  "double-word": "bg-board-double-word text-board-label",
  "triple-letter": "bg-board-triple-letter text-board-label",
  "triple-word": "bg-board-triple-word text-board-label",
}

const premiumCoordinates: Record<Premium, Array<[number, number]>> = {
  "triple-word": [
    [0, 0],
    [0, 7],
    [0, 14],
    [7, 0],
    [7, 14],
    [14, 0],
    [14, 7],
    [14, 14],
  ],
  "double-word": [
    [1, 1],
    [1, 13],
    [2, 2],
    [2, 12],
    [3, 3],
    [3, 11],
    [4, 4],
    [4, 10],
    [7, 7],
    [10, 4],
    [10, 10],
    [11, 3],
    [11, 11],
    [12, 2],
    [12, 12],
    [13, 1],
    [13, 13],
  ],
  "triple-letter": [
    [1, 5],
    [1, 9],
    [5, 1],
    [5, 5],
    [5, 9],
    [5, 13],
    [9, 1],
    [9, 5],
    [9, 9],
    [9, 13],
    [13, 5],
    [13, 9],
  ],
  "double-letter": [
    [0, 3],
    [0, 11],
    [2, 6],
    [2, 8],
    [3, 0],
    [3, 7],
    [3, 14],
    [6, 2],
    [6, 6],
    [6, 8],
    [6, 12],
    [7, 3],
    [7, 11],
    [8, 2],
    [8, 6],
    [8, 8],
    [8, 12],
    [11, 0],
    [11, 7],
    [11, 14],
    [12, 6],
    [12, 8],
    [14, 3],
    [14, 11],
  ],
}

const premiums = new Map<string, Premium>()
for (const [premium, coordinates] of Object.entries(
  premiumCoordinates
) as Array<[Premium, Array<[number, number]>]>) {
  for (const [row, column] of coordinates) {
    premiums.set(`${row}-${column}`, premium)
  }
}

const squares = Array.from({ length: BOARD_SIZE * BOARD_SIZE }, (_, index) => ({
  row: Math.floor(index / BOARD_SIZE),
  column: index % BOARD_SIZE,
}))

function squareName(row: number, column: number) {
  return `${String.fromCharCode(65 + column)}${row + 1}`
}

export function GameBoard({ tiles }: GameBoardProps) {
  return (
    <div className="mx-auto w-full max-w-[720px]">
      <div className="mb-1.5 grid grid-cols-[repeat(15,minmax(0,1fr))] px-1.5 text-center font-mono text-[9px] text-muted-foreground sm:text-[10px]">
        {Array.from({ length: BOARD_SIZE }, (_, index) => (
          <span key={index}>{String.fromCharCode(65 + index)}</span>
        ))}
      </div>
      <ol
        aria-label="15 by 15 word game board"
        className="grid grid-cols-[repeat(15,minmax(0,1fr))] gap-px rounded-xl bg-board-line p-1.5 shadow-inner ring-1 ring-foreground/10"
      >
        {squares.map(({ column, row }) => {
          const key = `${row}-${column}`
          const premium = premiums.get(key)
          const tile = tiles[key]
          const name = squareName(row, column)
          const description = tile
            ? `${name}: ${tile.letter}, ${tile.value} points${tile.recent ? ", part of the latest move" : ""}`
            : premium
              ? `${name}: ${premiumNames[premium]} score`
              : `${name}: empty`

          return (
            <li
              aria-label={description}
              className={cn(
                "relative aspect-square min-w-0 list-none overflow-hidden rounded-[3px] bg-board",
                premium && premiumClasses[premium]
              )}
              key={key}
            >
              {premium && !tile ? (
                <span
                  aria-hidden="true"
                  className="absolute inset-0 grid place-items-center font-heading text-[clamp(0.3rem,0.65vw,0.55rem)] font-semibold tracking-tight"
                >
                  {premiumLabels[premium]}
                </span>
              ) : null}
              {tile ? (
                <span
                  aria-hidden="true"
                  className={cn(
                    "absolute inset-[7%] grid place-items-center rounded-[18%] bg-tile font-heading text-[clamp(0.55rem,1.4vw,1.05rem)] font-semibold text-tile-foreground shadow-[inset_0_-2px_0_var(--tile-edge),0_1px_2px_oklch(0_0_0/18%)]",
                    tile.recent && "ring-2 ring-primary ring-inset"
                  )}
                >
                  {tile.letter}
                  <span className="absolute right-[9%] bottom-[4%] text-[clamp(0.28rem,0.5vw,0.45rem)] leading-none font-medium">
                    {tile.value}
                  </span>
                </span>
              ) : null}
            </li>
          )
        })}
      </ol>
      <div className="mt-2 flex items-center justify-center gap-4 text-[10px] text-muted-foreground sm:text-xs">
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
  )
}
