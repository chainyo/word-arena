import { LockKeyhole } from "lucide-react"

import { Badge } from "@/components/ui/badge"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { cn } from "@/lib/utils"

export type RackTile = {
  id: number
  letter: string
  value: number
}

type GameRackProps = {
  disabled?: boolean
  exchangeIds?: number[]
  label: string
  mode?: "place" | "exchange" | "read_only"
  onPlacedTileSelect?: (tileId: number) => void
  onTileSelect?: (tileId: number) => void
  placedIds?: number[]
  selectedTileId?: number
  tiles: RackTile[]
}

export function GameRack({
  disabled = false,
  exchangeIds = [],
  label,
  mode = "read_only",
  onPlacedTileSelect,
  onTileSelect,
  placedIds = [],
  selectedTileId,
  tiles,
}: GameRackProps) {
  const interactive = mode !== "read_only" && onTileSelect !== undefined
  const rackDescription = tiles
    .map((tile) => (tile.letter === "?" ? "blank" : tile.letter))
    .join(", ")
  return (
    <Card className="mt-3" size="sm">
      <CardHeader className="border-b">
        <div className="flex flex-wrap items-center gap-2">
          <CardTitle>{label}</CardTitle>
          <Badge className="gap-1" variant="outline">
            <LockKeyhole className="size-3" /> Seat-private
          </Badge>
        </div>
        <CardDescription>
          {mode === "exchange"
            ? "Choose one or more tiles to exchange"
            : mode === "place"
              ? "Choose a tile, then choose an open board square"
              : "Current authoritative rack"}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <ol
          aria-label={`${label}: ${rackDescription || "empty"}`}
          className="flex min-h-12 flex-wrap items-center justify-center gap-1.5 sm:gap-2"
        >
          {tiles.map((tile) => {
            const selected =
              selectedTileId === tile.id || exchangeIds.includes(tile.id)
            const placed = placedIds.includes(tile.id)
            const labelText = `${tile.letter === "?" ? "blank" : tile.letter}, ${tile.value} points${placed ? ", staged on board; activate to return to rack" : selected ? ", selected" : ""}`
            return (
              <li key={tile.id}>
                {interactive ? (
                  <button
                    aria-label={labelText}
                    aria-pressed={selected || placed}
                    className={cn(
                      "relative grid size-[clamp(2.75rem,11vw,3.25rem)] touch-manipulation place-items-center rounded-lg bg-tile font-heading text-lg font-semibold text-tile-foreground shadow-[inset_0_-3px_0_var(--tile-edge),0_2px_4px_var(--tile-shadow)] outline-none transition-transform focus-visible:ring-3 focus-visible:ring-ring/50 motion-reduce:transition-none sm:text-xl",
                      selected && "-translate-y-1 ring-2 ring-primary",
                      placed && "opacity-55 ring-2 ring-primary ring-dashed"
                    )}
                    disabled={disabled}
                    onClick={() =>
                      placed
                        ? onPlacedTileSelect?.(tile.id)
                        : onTileSelect(tile.id)
                    }
                    type="button"
                  >
                    <span aria-hidden="true">{tile.letter}</span>
                    <span
                      aria-hidden="true"
                      className="absolute right-1 bottom-1 text-[8px] leading-none font-medium"
                    >
                      {tile.value}
                    </span>
                  </button>
                ) : (
                  <span
                    aria-label={labelText}
                    className="relative grid size-[clamp(2.75rem,11vw,3.25rem)] place-items-center rounded-lg bg-tile font-heading text-lg font-semibold text-tile-foreground shadow-[inset_0_-3px_0_var(--tile-edge),0_2px_4px_var(--tile-shadow)] sm:text-xl"
                    role="img"
                  >
                    <span aria-hidden="true">{tile.letter}</span>
                    <span
                      aria-hidden="true"
                      className="absolute right-1 bottom-1 text-[8px] leading-none font-medium"
                    >
                      {tile.value}
                    </span>
                  </span>
                )}
              </li>
            )
          })}
        </ol>
      </CardContent>
    </Card>
  )
}
