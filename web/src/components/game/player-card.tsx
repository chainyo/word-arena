import { Bot, UserRound } from "lucide-react"

import type { AgentHarnessId } from "@/api/types"
import { AgentLogo } from "@/components/agent/agent-logo"
import { GameClock } from "@/components/game/game-clock"
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
  harness?: AgentHarnessId
  human?: boolean
  deadlineAt?: number
  observedAt?: number
  score: number
  subtitle: string
  status?: string
}

export function PlayerCard({
  active = false,
  agent,
  harness,
  human = false,
  deadlineAt,
  observedAt,
  score,
  subtitle,
  status,
}: PlayerCardProps) {
  return (
    <Card
      aria-current={active ? "true" : undefined}
      className={cn(active && "ring-primary/45")}
      size="sm"
    >
      <CardHeader>
        <div className="flex min-w-0 items-center gap-2.5">
          {harness ? (
            <AgentLogo
              agent={harness}
              className={cn(
                "shrink-0",
                active && "border-primary bg-primary text-primary-foreground"
              )}
            />
          ) : (
            <span
              className={cn(
                "grid size-9 shrink-0 place-items-center rounded-lg bg-secondary text-secondary-foreground",
                active && "bg-primary text-primary-foreground"
              )}
            >
              {human ? (
                <UserRound className="size-4" />
              ) : (
                <Bot className="size-4" />
              )}
            </span>
          )}
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
        <GameClock
          active={active}
          deadlineAt={deadlineAt}
          label={agent}
          observedAt={observedAt}
        />
        {status ? (
          <Badge variant={active ? "default" : "outline"}>{status}</Badge>
        ) : active ? (
          <Badge>Your turn</Badge>
        ) : (
          <Badge variant="outline">Waiting</Badge>
        )}
      </CardContent>
    </Card>
  )
}
