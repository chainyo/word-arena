import { Terminal } from "lucide-react"

import type { AgentActivityEvent, AgentMatchActivity, Seat } from "@/api/types"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { cn } from "@/lib/utils"

type AgentConsoleProps = {
  activity?: AgentMatchActivity
  activeSeat?: Seat
  now?: number
  seatNames?: [string, string]
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
  activeSeat = "one",
  now = Date.now(),
  seatNames = ["Seat one", "Seat two"],
}: AgentConsoleProps) {
  const events = activity?.events ?? []

  const log = (seat?: Seat) => {
    const recent = events
      .filter((event) =>
        seat ? event.seat === seat : event.seat === undefined
      )
      .slice(-24)
      .reverse()

    return (
      <ol
        aria-label={
          seat ? `Seat ${seat} agent activity log` : "Match activity log"
        }
        aria-live="polite"
        className="divide-y"
      >
        {recent.length === 0 ? (
          <li className="px-3 py-6 text-center font-sans text-muted-foreground">
            {seat
              ? `Waiting for ${seatNames[seat === "one" ? 0 : 1]} activity.`
              : "Waiting for match activity."}
          </li>
        ) : null}
        {recent.map((event) => {
          const duration = eventDuration(event, events, now)
          return (
            <li className="space-y-1.5 px-3 py-2.5" key={event.sequence}>
              <div className="flex items-center gap-2 text-[10px] text-muted-foreground">
                <span className="tabular-nums">{eventTime(event)}</span>
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
    )
  }

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
      <CardContent className="px-0 pt-0 font-mono text-xs">
        <Tabs defaultValue={activeSeat}>
          <TabsList className="mx-3 mt-3 grid h-auto w-[calc(100%-1.5rem)] grid-cols-3">
            {(["one", "two"] as const).map((seat, index) => (
              <TabsTrigger
                className={cn(
                  "min-w-0 flex-col gap-0 py-1.5 leading-tight text-foreground/80 dark:text-foreground/80",
                  seat === "one"
                    ? "data-active:bg-seat-one/20"
                    : "data-active:bg-seat-two/20"
                )}
                key={seat}
                value={seat}
              >
                <span className="flex max-w-full items-center gap-1.5">
                  <span
                    aria-hidden="true"
                    className={cn(
                      "size-2 shrink-0 rounded-full",
                      seat === "one" ? "bg-seat-one" : "bg-seat-two"
                    )}
                  />
                  <span>Seat {index + 1}</span>
                </span>
                <span className="max-w-full truncate text-[10px] font-normal text-muted-foreground">
                  {seatNames[index]}
                </span>
              </TabsTrigger>
            ))}
            <TabsTrigger value="match">Match</TabsTrigger>
          </TabsList>
          <div className="mt-3 max-h-96 overflow-y-auto border-t">
            <TabsContent value="one">{log("one")}</TabsContent>
            <TabsContent value="two">{log("two")}</TabsContent>
            <TabsContent value="match">{log()}</TabsContent>
          </div>
        </Tabs>
      </CardContent>
    </Card>
  )
}
