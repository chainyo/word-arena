import { Terminal } from "lucide-react"

import type { AgentActivityEvent, AgentMatchActivity } from "@/api/types"
import { Badge } from "@/components/ui/badge"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"

type AgentConsoleProps = {
  activity?: AgentMatchActivity
  now?: number
}

function elapsedLabel(milliseconds: number) {
  const seconds = Math.max(0, Math.floor(milliseconds / 1_000))
  const minutes = Math.floor(seconds / 60)
  const remainder = seconds % 60
  return minutes > 0 ? `${minutes}m ${remainder}s` : `${remainder}s`
}

function eventTime(event: AgentActivityEvent) {
  return new Date(event.atUnixMs).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  })
}

function activeTurn(event: AgentActivityEvent, events: AgentActivityEvent[]) {
  if (event.kind !== "turn_started" || !event.turnId) return false
  return !events.some(
    (candidate) =>
      candidate.sequence > event.sequence &&
      candidate.turnId === event.turnId &&
      (candidate.kind === "turn_completed" || candidate.kind === "turn_failed")
  )
}

function eventDuration(
  event: AgentActivityEvent,
  events: AgentActivityEvent[],
  now: number
) {
  if (event.durationMs !== undefined) return elapsedLabel(event.durationMs)
  if (activeTurn(event, events))
    return `${elapsedLabel(now - event.atUnixMs)} live`
  return undefined
}

export function AgentConsole({
  activity,
  now = Date.now(),
}: AgentConsoleProps) {
  const events = activity?.events ?? []
  const recent = events.slice(-24).reverse()

  return (
    <Card size="sm">
      <CardHeader className="border-b">
        <div className="flex items-center gap-2">
          <Terminal aria-hidden="true" className="size-4" />
          <CardTitle>Agent activity</CardTitle>
        </div>
        <CardDescription>
          Redacted lifecycle, tools, output, and failures
        </CardDescription>
      </CardHeader>
      <CardContent className="max-h-96 overflow-y-auto px-0 font-mono text-xs">
        <ol
          aria-label="Agent activity log"
          aria-live="polite"
          className="divide-y"
        >
          {recent.length === 0 ? (
            <li className="px-3 py-6 text-center font-sans text-muted-foreground">
              Waiting for agent activity.
            </li>
          ) : null}
          {recent.map((event) => {
            const duration = eventDuration(event, events, now)
            return (
              <li className="space-y-1.5 px-3 py-2.5" key={event.sequence}>
                <div className="flex items-center gap-2 text-[10px] text-muted-foreground">
                  <span className="tabular-nums">{eventTime(event)}</span>
                  {event.seat ? (
                    <Badge className="font-sans" variant="outline">
                      Seat {event.seat}
                    </Badge>
                  ) : null}
                  <span>{event.kind.replaceAll("_", " ")}</span>
                  {duration ? (
                    <span className="ml-auto tabular-nums">{duration}</span>
                  ) : null}
                </div>
                <p className="whitespace-pre-wrap break-words leading-5">
                  {event.message}
                </p>
              </li>
            )
          })}
        </ol>
      </CardContent>
    </Card>
  )
}
