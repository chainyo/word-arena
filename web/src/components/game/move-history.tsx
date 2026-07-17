import { Badge } from "@/components/ui/badge"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"

export type MoveRecord = {
  player: string
  turn: number
  word: string
  score: number
  detail: string
  elapsed: string
}

type MoveHistoryProps = {
  moves: MoveRecord[]
}

export function MoveHistory({ moves }: MoveHistoryProps) {
  return (
    <Card size="sm">
      <CardHeader className="border-b">
        <CardTitle>Move history</CardTitle>
        <CardDescription>Newest public event first</CardDescription>
      </CardHeader>
      <CardContent className="px-0">
        <ol className="divide-y" aria-label="Recent moves">
          {moves.map((move, index) => (
            <li className="px-3 py-3" key={move.turn}>
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <span className="truncate font-medium">{move.player}</span>
                    {index === 0 ? (
                      <Badge variant="secondary">Latest</Badge>
                    ) : null}
                  </div>
                  <div className="mt-1 flex items-baseline gap-2">
                    <span className="font-heading text-base font-semibold tracking-wide">
                      {move.word}
                    </span>
                    <span className="text-xs text-muted-foreground">
                      {move.detail}
                    </span>
                  </div>
                </div>
                <span className="font-heading text-lg font-semibold tabular-nums">
                  +{move.score}
                </span>
              </div>
              <div className="mt-2 flex items-center justify-between text-[11px] text-muted-foreground">
                <span>Turn {move.turn}</span>
                <span>{move.elapsed}</span>
              </div>
            </li>
          ))}
        </ol>
      </CardContent>
    </Card>
  )
}
