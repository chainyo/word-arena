import { LockKeyhole } from "lucide-react"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"

type RackTile = {
  letter: string
  value: number
}

type GameRackProps = {
  tiles: RackTile[]
}

export function GameRack({ tiles }: GameRackProps) {
  return (
    <Card className="mt-3" size="sm">
      <CardHeader className="border-b">
        <div className="flex flex-wrap items-center gap-2">
          <CardTitle>Codex rack</CardTitle>
          <Badge className="gap-1" variant="outline">
            <LockKeyhole className="size-3" /> Seat-private
          </Badge>
        </div>
        <CardDescription>
          Hidden from the opponent and public spectators
        </CardDescription>
      </CardHeader>
      <CardContent>
        <ol
          aria-label="Codex rack: A, I, N, R, S, T, blank"
          className="flex items-center justify-center gap-1.5 sm:gap-2"
        >
          {tiles.map((tile, index) => (
            <li
              aria-label={`${tile.letter === "?" ? "blank" : tile.letter}, ${tile.value} points`}
              className="relative grid size-[clamp(2.25rem,10vw,3rem)] shrink-0 place-items-center rounded-lg bg-tile font-heading text-lg font-semibold text-tile-foreground shadow-[inset_0_-3px_0_var(--tile-edge),0_2px_4px_oklch(0_0_0/18%)] sm:text-xl"
              key={`${tile.letter}-${index}`}
            >
              <span aria-hidden="true">{tile.letter}</span>
              <span
                aria-hidden="true"
                className="absolute right-1 bottom-1 text-[8px] leading-none font-medium"
              >
                {tile.value}
              </span>
            </li>
          ))}
        </ol>
      </CardContent>
      <CardFooter className="flex-wrap justify-between gap-2">
        <p className="text-xs text-muted-foreground">
          Actions unlock when the referee API is connected.
        </p>
        <div className="flex gap-2">
          <Button disabled size="sm" variant="outline">
            Exchange
          </Button>
          <Button disabled size="sm">
            Play move
          </Button>
        </div>
      </CardFooter>
    </Card>
  )
}
