import { Bot, Clock3 } from "lucide-react"

import { Badge } from "@/components/ui/badge"
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { cn } from "@/lib/utils"

type PlayerCardProps = {
  active?: boolean
  agent: string
  clock: string
  score: number
  subtitle: string
}

export function PlayerCard({
  active = false,
  agent,
  clock,
  score,
  subtitle,
}: PlayerCardProps) {
  return (
    <Card
      aria-current={active ? "true" : undefined}
      className={cn(active && "ring-primary/45")}
      size="sm"
    >
      <CardHeader>
        <div className="flex min-w-0 items-center gap-2.5">
          <span
            className={cn(
              "grid size-9 shrink-0 place-items-center rounded-lg bg-secondary text-secondary-foreground",
              active && "bg-primary text-primary-foreground"
            )}
          >
            <Bot className="size-4" />
          </span>
          <div className="min-w-0">
            <CardTitle className="truncate">{agent}</CardTitle>
            <CardDescription className="truncate">{subtitle}</CardDescription>
          </div>
        </div>
        <CardAction className="text-right">
          <p className="font-heading text-2xl leading-none font-semibold tabular-nums">
            {score}
          </p>
          <p className="mt-1 text-[10px] tracking-wide text-muted-foreground uppercase">
            points
          </p>
        </CardAction>
      </CardHeader>
      <CardContent className="flex items-center justify-between gap-2">
        <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <Clock3 className="size-3.5" />
          <span className="font-mono tabular-nums">{clock}</span>
        </span>
        {active ? (
          <Badge>Your turn</Badge>
        ) : (
          <Badge variant="outline">Waiting</Badge>
        )}
      </CardContent>
    </Card>
  )
}
