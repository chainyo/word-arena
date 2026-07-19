import {
  ChevronFirst,
  ChevronLast,
  ChevronLeft,
  ChevronRight,
  Download,
  Pause,
  Play,
  Share2,
} from "lucide-react"
import { useEffect, useMemo, useState } from "react"

import type { ReplayBundle } from "@/api/types"
import {
  displayLetterValues,
  displayPremiums,
} from "@/components/game/display-rules"
import { type BoardTile, GameBoard } from "@/components/game/game-board"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Input } from "@/components/ui/input"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Separator } from "@/components/ui/separator"
import {
  filterReplayEvents,
  formatStatistic,
  replayFrame,
  replayStatistics,
  seedHex,
  serializePublicReplay,
} from "@/replay"

const PAGE_SIZE = 8

function eventLabel(type: string): string {
  return type.replaceAll("_", " ")
}

function downloadReplay(gameId: string, replay: ReplayBundle) {
  const blob = new Blob([serializePublicReplay(replay)], {
    type: "application/json",
  })
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement("a")
  anchor.href = url
  anchor.download = `${gameId}-public-replay.json`
  anchor.click()
  URL.revokeObjectURL(url)
}

export function ReplayView({
  gameId,
  onShare,
  replay,
}: {
  gameId: string
  onShare?: () => void
  replay: ReplayBundle
}) {
  const lastSequence = replay.events.at(-1)?.sequence ?? 0
  const [sequence, setSequence] = useState(lastSequence)
  const [playing, setPlaying] = useState(false)
  const [query, setQuery] = useState("")
  const [kind, setKind] = useState("all")
  const [page, setPage] = useState(0)
  const frame = useMemo(() => replayFrame(replay, sequence), [replay, sequence])
  const statistics = useMemo(() => replayStatistics(replay), [replay])
  const filtered = useMemo(
    () => filterReplayEvents(replay.events, query, kind),
    [kind, query, replay.events]
  )
  const pageCount = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE))
  const pageEvents = filtered.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE)

  useEffect(() => {
    if (!playing) return undefined
    if (sequence >= lastSequence) {
      setPlaying(false)
      return undefined
    }
    const timer = window.setTimeout(
      () => setSequence((current) => Math.min(lastSequence, current + 1)),
      700
    )
    return () => window.clearTimeout(timer)
  }, [lastSequence, playing, sequence])

  const values = displayLetterValues(replay.ruleset.id, replay.ruleset)
  const tiles: Record<string, BoardTile> = {}
  frame.board.forEach((tile, index) => {
    if (!tile) return
    tiles[`${Math.floor(index / 15)}-${index % 15}`] = {
      letter: tile.letter,
      value: tile.is_blank ? 0 : values.get(tile.letter),
      recent:
        frame.event?.kind.type === "move_played" &&
        Array.isArray(frame.event.kind.placements) &&
        frame.event.kind.placements.some((value) => {
          if (typeof value !== "object" || value === null) return false
          const placement = value as { coordinate?: unknown }
          if (
            typeof placement.coordinate !== "object" ||
            placement.coordinate === null
          )
            return false
          const coordinate = placement.coordinate as {
            row?: unknown
            column?: unknown
          }
          return (
            coordinate.row === Math.floor(index / 15) &&
            coordinate.column === index % 15
          )
        }),
    }
  })

  return (
    <main className="mx-auto grid max-w-[1600px] items-start gap-3 p-3 sm:p-5 xl:grid-cols-[minmax(0,1fr)_22rem]">
      <section className="min-w-0 space-y-3">
        <Card size="sm">
          <CardHeader className="border-b sm:grid-cols-[1fr_auto]">
            <div>
              <div className="mb-1 flex items-center gap-2">
                <Badge variant="secondary">Recorded replay</Badge>
                <span className="text-xs text-muted-foreground">
                  Event {frame.sequence + 1} of {replay.events.length}
                </span>
              </div>
              <CardTitle>Post-game board</CardTitle>
              <CardDescription>
                Immutable event playback; the live referee is never mutated.
              </CardDescription>
            </div>
            <div className="flex flex-wrap gap-1 self-center">
              <Button
                aria-label="First replay event"
                onClick={() => setSequence(0)}
                size="icon-sm"
                variant="outline"
              >
                <ChevronFirst />
              </Button>
              <Button
                aria-label="Previous replay event"
                disabled={sequence <= 0}
                onClick={() =>
                  setSequence((current) => Math.max(0, current - 1))
                }
                size="icon-sm"
                variant="outline"
              >
                <ChevronLeft />
              </Button>
              <Button
                aria-label={playing ? "Pause replay" : "Play replay"}
                onClick={() => {
                  if (sequence >= lastSequence) setSequence(0)
                  setPlaying((current) => !current)
                }}
                size="icon-sm"
              >
                {playing ? <Pause /> : <Play />}
              </Button>
              <Button
                aria-label="Next replay event"
                disabled={sequence >= lastSequence}
                onClick={() =>
                  setSequence((current) => Math.min(lastSequence, current + 1))
                }
                size="icon-sm"
                variant="outline"
              >
                <ChevronRight />
              </Button>
              <Button
                aria-label="Last replay event"
                onClick={() => setSequence(lastSequence)}
                size="icon-sm"
                variant="outline"
              >
                <ChevronLast />
              </Button>
            </div>
          </CardHeader>
          <CardContent>
            <GameBoard
              premiums={displayPremiums(replay.ruleset)}
              tiles={tiles}
            />
          </CardContent>
        </Card>
        <Card size="sm">
          <CardHeader className="border-b">
            <CardTitle>Recorded events</CardTitle>
            <CardDescription>
              Search and filter {replay.events.length} immutable public events.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            <div className="grid gap-2 sm:grid-cols-[1fr_13rem]">
              <Input
                aria-label="Search replay events"
                onChange={(event) => {
                  setQuery(event.target.value)
                  setPage(0)
                }}
                placeholder="Search word or event"
                value={query}
              />
              <Select
                onValueChange={(value) => {
                  if (value) {
                    setKind(value)
                    setPage(0)
                  }
                }}
                value={kind}
              >
                <SelectTrigger aria-label="Filter replay event type">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">All events</SelectItem>
                  <SelectItem value="move_played">Moves</SelectItem>
                  <SelectItem value="passed">Passes</SelectItem>
                  <SelectItem value="exchanged">Exchanges</SelectItem>
                  <SelectItem value="resigned">Resignations</SelectItem>
                </SelectContent>
              </Select>
            </div>
            {pageEvents.length ? (
              <ol className="divide-y rounded-lg border">
                {pageEvents.map((event) => (
                  <li
                    className="flex items-center justify-between gap-3 p-3 text-sm"
                    key={event.sequence}
                  >
                    <button
                      className="text-left font-medium capitalize underline-offset-4 hover:underline"
                      onClick={() => setSequence(event.sequence)}
                      type="button"
                    >
                      {event.sequence}. {eventLabel(event.kind.type)}
                    </button>
                    <Badge variant="outline">public</Badge>
                  </li>
                ))}
              </ol>
            ) : (
              <p className="rounded-lg border border-dashed p-6 text-center text-sm text-muted-foreground">
                No recorded events match these filters.
              </p>
            )}
            <div className="flex items-center justify-between gap-3 text-xs text-muted-foreground">
              <span>
                Page {page + 1} of {pageCount}
              </span>
              <div className="flex gap-1">
                <Button
                  disabled={page === 0}
                  onClick={() => setPage((current) => Math.max(0, current - 1))}
                  size="sm"
                  variant="outline"
                >
                  Previous
                </Button>
                <Button
                  disabled={page + 1 >= pageCount}
                  onClick={() =>
                    setPage((current) => Math.min(pageCount - 1, current + 1))
                  }
                  size="sm"
                  variant="outline"
                >
                  Next
                </Button>
              </div>
            </div>
          </CardContent>
        </Card>
      </section>
      <aside className="space-y-3">
        <Card size="sm">
          <CardHeader className="border-b">
            <CardTitle>Score at event {frame.sequence}</CardTitle>
            <CardDescription>{frame.phase} recorded state</CardDescription>
          </CardHeader>
          <CardContent className="grid grid-cols-2 gap-3 text-center">
            <div className="rounded-lg border p-3">
              <p className="text-xs text-muted-foreground">Seat one</p>
              <p className="font-heading text-2xl font-semibold">
                {frame.scores[0]}
              </p>
            </div>
            <div className="rounded-lg border p-3">
              <p className="text-xs text-muted-foreground">Seat two</p>
              <p className="font-heading text-2xl font-semibold">
                {frame.scores[1]}
              </p>
            </div>
          </CardContent>
        </Card>
        <Card size="sm">
          <CardHeader className="border-b">
            <CardTitle>Match statistics</CardTitle>
            <CardDescription>Derived from authoritative events</CardDescription>
          </CardHeader>
          <CardContent className="space-y-2 text-sm">
            {[
              ["Turns", formatStatistic(statistics.turns)],
              ["Move score", formatStatistic(statistics.moveScore)],
              [
                "Average move",
                formatStatistic(statistics.averageMoveScore, "decimal"),
              ],
              ["Bingos", formatStatistic(statistics.bingos)],
              ["Passes", formatStatistic(statistics.passes)],
              ["Exchanges", formatStatistic(statistics.exchanges)],
              ["Vocabulary", formatStatistic(statistics.uniqueWords)],
            ].map(([label, value]) => (
              <div className="flex justify-between gap-3" key={label}>
                <span className="text-muted-foreground">{label}</span>
                <span className="font-medium">{value}</span>
              </div>
            ))}
          </CardContent>
        </Card>
        <Card size="sm">
          <CardHeader className="border-b">
            <CardTitle>Exact replay inputs</CardTitle>
            <CardDescription>Available only after completion</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3 text-xs">
            <div>
              <p className="text-muted-foreground">Ruleset identity</p>
              <p className="break-all font-mono">
                {replay.rulesetIdentity.rulesetId}@
                {replay.rulesetIdentity.contentSha256}
              </p>
            </div>
            <div>
              <p className="text-muted-foreground">Lexicon pack</p>
              <p className="break-all font-mono">
                {replay.lexicon.packId}@{replay.lexicon.packVersion} ·{" "}
                {replay.lexicon.contentSha256}
              </p>
            </div>
            <div>
              <p className="text-muted-foreground">RNG / seed reveal</p>
              <p className="break-all font-mono">
                {replay.rngAlgorithm} · {seedHex(replay.seedReveal)}
              </p>
            </div>
            <Separator />
            <p className="text-muted-foreground">
              This authorized view records {replay.privateEvents.length} private
              transitions. Public export always removes them.
            </p>
            <div className="grid gap-2 sm:grid-cols-2 xl:grid-cols-1">
              <Button
                onClick={() => downloadReplay(gameId, replay)}
                variant="outline"
              >
                <Download /> Export public replay
              </Button>
              <Button onClick={onShare} variant="outline">
                <Share2 /> Copy public route
              </Button>
            </div>
          </CardContent>
        </Card>
      </aside>
    </main>
  )
}
