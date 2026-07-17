import { Clock3, Languages, Layers3, Moon, Radio, Sun } from "lucide-react"

import { type BoardTile, GameBoard } from "@/components/game/game-board"
import { GameRack } from "@/components/game/game-rack"
import { MoveHistory, type MoveRecord } from "@/components/game/move-history"
import { PlayerCard } from "@/components/game/player-card"
import { useTheme } from "@/components/theme-provider"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Separator } from "@/components/ui/separator"

const boardTiles: Record<string, BoardTile> = {
  "6-6": { letter: "W", value: 4, recent: true },
  "7-5": { letter: "C", value: 3 },
  "7-6": { letter: "O", value: 1 },
  "7-7": { letter: "D", value: 2 },
  "7-8": { letter: "E", value: 1 },
  "7-9": { letter: "X", value: 8 },
  "8-6": { letter: "R", value: 1, recent: true },
  "9-6": { letter: "D", value: 2, recent: true },
  "10-6": { letter: "S", value: 1, recent: true },
}

const rack = [
  { id: "rack-a", letter: "A", value: 1 },
  { id: "rack-i", letter: "I", value: 1 },
  { id: "rack-n", letter: "N", value: 1 },
  { id: "rack-r", letter: "R", value: 1 },
  { id: "rack-s", letter: "S", value: 1 },
  { id: "rack-t", letter: "T", value: 1 },
  { id: "rack-blank", letter: "?", value: 0 },
]

const moves: MoveRecord[] = [
  {
    player: "Claude Code",
    turn: 11,
    word: "WORDS",
    score: 24,
    detail: "G7 · down",
    elapsed: "18.4s",
  },
  {
    player: "Codex",
    turn: 10,
    word: "CODEX",
    score: 42,
    detail: "F8 · across",
    elapsed: "11.2s",
  },
  {
    player: "Claude Code",
    turn: 9,
    word: "PASS",
    score: 0,
    detail: "No tiles played",
    elapsed: "7.8s",
  },
  {
    player: "Codex",
    turn: 8,
    word: "TOOLS",
    score: 31,
    detail: "J4 · down",
    elapsed: "14.1s",
  },
]

export function App() {
  const { setTheme, theme } = useTheme()
  const systemThemeIsDark = window.matchMedia(
    "(prefers-color-scheme: dark)"
  ).matches
  const currentThemeIsDark =
    theme === "dark" || (theme === "system" && systemThemeIsDark)
  const nextTheme = currentThemeIsDark ? "light" : "dark"

  return (
    <div className="min-h-svh bg-background">
      <header className="sticky top-0 z-20 border-b bg-background/95 backdrop-blur-sm">
        <div className="mx-auto flex h-14 max-w-[1600px] items-center justify-between gap-3 px-3 sm:px-5">
          <div className="flex min-w-0 items-center gap-3">
            <span className="grid size-8 shrink-0 place-items-center rounded-lg bg-primary font-heading text-xs font-semibold text-primary-foreground shadow-sm">
              WA
            </span>
            <div className="min-w-0">
              <p className="truncate font-heading text-sm font-medium tracking-tight">
                Word Arena
              </p>
              <p className="truncate text-[11px] text-muted-foreground">
                Game 8AF3 · Codex seat
              </p>
            </div>
            <Badge className="hidden sm:inline-flex" variant="secondary">
              Local preview
            </Badge>
          </div>

          <div className="flex shrink-0 items-center gap-2">
            <Badge className="hidden gap-1.5 sm:inline-flex" variant="outline">
              <span className="size-1.5 rounded-full bg-primary" />
              Turn 12
            </Badge>
            <Button
              aria-label={`Switch to ${nextTheme} theme`}
              onClick={() => setTheme(nextTheme)}
              size="icon"
              variant="outline"
            >
              {currentThemeIsDark ? <Sun /> : <Moon />}
            </Button>
          </div>
        </div>
      </header>

      <main className="mx-auto grid max-w-[1600px] items-start gap-3 p-3 sm:p-5 lg:grid-cols-[minmax(0,1fr)_18rem] xl:grid-cols-[15rem_minmax(0,1fr)_18rem]">
        <aside
          aria-label="Players and match details"
          className="grid gap-3 sm:grid-cols-2 lg:col-span-2 xl:col-span-1 xl:grid-cols-1"
        >
          <PlayerCard
            active
            agent="Codex"
            clock="08:42"
            score={148}
            subtitle="Current seat · 7 tiles"
          />
          <PlayerCard
            agent="Claude Code"
            clock="07:58"
            score={132}
            subtitle="Opponent · 7 tiles"
          />

          <Card className="sm:col-span-2 xl:col-span-1" size="sm">
            <CardHeader className="border-b">
              <CardTitle>Match</CardTitle>
              <CardDescription>Public game configuration</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <div className="flex items-center justify-between gap-3">
                <span className="flex items-center gap-2 text-muted-foreground">
                  <Languages className="size-4" /> Language
                </span>
                <span className="font-medium">English</span>
              </div>
              <div className="flex items-center justify-between gap-3">
                <span className="flex items-center gap-2 text-muted-foreground">
                  <Layers3 className="size-4" /> Bag
                </span>
                <span className="font-medium">54 tiles</span>
              </div>
              <div className="flex items-center justify-between gap-3">
                <span className="flex items-center gap-2 text-muted-foreground">
                  <Clock3 className="size-4" /> Clock
                </span>
                <span className="font-medium">10 min</span>
              </div>
              <Separator />
              <div className="flex items-center justify-between gap-3 text-xs">
                <span className="text-muted-foreground">Ruleset</span>
                <code className="rounded bg-muted px-1.5 py-0.5">
                  classic-en@1
                </code>
              </div>
            </CardContent>
          </Card>
        </aside>

        <section
          aria-labelledby="board-heading"
          className="min-w-0 lg:col-start-1 xl:col-start-2"
        >
          <Card className="gap-3" size="sm">
            <CardHeader className="border-b sm:grid-cols-[1fr_auto]">
              <div>
                <div className="mb-1 flex items-center gap-2">
                  <Badge>Codex to move</Badge>
                  <span className="text-xs text-muted-foreground">Turn 12</span>
                </div>
                <CardTitle id="board-heading">Live board</CardTitle>
                <CardDescription>
                  Last play: WORDS for 24 points
                </CardDescription>
              </div>
              <div className="hidden items-center gap-2 self-center text-xs text-muted-foreground sm:flex">
                <Radio className="size-3.5 text-primary" />
                Seat-private view
              </div>
            </CardHeader>
            <CardContent>
              <GameBoard tiles={boardTiles} />
            </CardContent>
          </Card>

          <GameRack tiles={rack} />
        </section>

        <aside
          aria-label="Game activity"
          className="min-w-0 lg:col-start-2 lg:row-start-2 xl:col-start-3 xl:row-start-1"
        >
          <MoveHistory moves={moves} />

          <Card className="mt-3" size="sm">
            <CardHeader className="border-b">
              <CardTitle>Referee connection</CardTitle>
              <CardDescription>Local Rust server</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <div className="flex items-center justify-between gap-3">
                <span className="flex items-center gap-2 text-muted-foreground">
                  <span className="size-2 rounded-full bg-muted-foreground/50" />
                  Game API
                </span>
                <Badge variant="outline">Not connected</Badge>
              </div>
              <p className="text-xs leading-5 text-muted-foreground">
                This preview becomes live when the game snapshot and event APIs
                are implemented.
              </p>
            </CardContent>
          </Card>
        </aside>
      </main>
    </div>
  )
}

export default App
