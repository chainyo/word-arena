import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import {
  AlertCircle,
  Languages,
  Layers3,
  Moon,
  Radio,
  RefreshCw,
  Sun,
  Unplug,
} from "lucide-react"
import {
  Component,
  type ErrorInfo,
  type FormEvent,
  type ReactNode,
  useEffect,
  useRef,
  useState,
} from "react"
import {
  createBrowserRouter,
  isRouteErrorResponse,
  Navigate,
  RouterProvider,
  useNavigate,
  useParams,
  useRouteError,
} from "react-router-dom"

import { DEFAULT_SERVER_ORIGIN, normalizeServerOrigin } from "@/api/client"
import { credentialVault } from "@/api/credentials"
import { gameQueryKey, gameQueryOptions } from "@/api/query"
import type {
  ConnectionState,
  GameAuthority,
  GameSession,
  GameView,
} from "@/api/types"
import { connectInvalidationSocket } from "@/api/websocket"
import { type BoardTile, GameBoard } from "@/components/game/game-board"
import { MoveHistory, type MoveRecord } from "@/components/game/move-history"
import { PlayerCard } from "@/components/game/player-card"
import { useTheme } from "@/components/theme-provider"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
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
import { Label } from "@/components/ui/label"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Separator } from "@/components/ui/separator"
import { Skeleton } from "@/components/ui/skeleton"

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { refetchOnWindowFocus: true, refetchOnReconnect: true },
  },
})

const placeholderSquares = Array.from(
  { length: 225 },
  (_, index) => `square-${Math.floor(index / 15)}-${index % 15}`
)

type AppErrorBoundaryState = { error?: Error }

class AppErrorBoundary extends Component<
  { children: ReactNode },
  AppErrorBoundaryState
> {
  state: AppErrorBoundaryState = {}

  static getDerivedStateFromError(error: Error): AppErrorBoundaryState {
    return { error }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Word Arena render failure", error, info.componentStack)
  }

  render() {
    if (this.state.error) {
      return (
        <FatalError
          message={this.state.error.message}
          onRetry={() => this.setState({ error: undefined })}
        />
      )
    }
    return this.props.children
  }
}

function FatalError({
  message,
  onRetry,
}: {
  message: string
  onRetry: () => void
}) {
  return (
    <main className="grid min-h-svh place-items-center bg-background p-4">
      <Card className="w-full max-w-lg">
        <CardHeader>
          <CardTitle>Game workspace stopped</CardTitle>
          <CardDescription>{message}</CardDescription>
        </CardHeader>
        <CardContent>
          <Button onClick={onRetry}>
            <RefreshCw /> Try again
          </Button>
        </CardContent>
      </Card>
    </main>
  )
}

function RouteError() {
  const error = useRouteError()
  const message = isRouteErrorResponse(error)
    ? `${error.status} ${error.statusText}`
    : error instanceof Error
      ? error.message
      : "The requested game route is invalid"
  return (
    <FatalError message={message} onRetry={() => window.location.assign("/")} />
  )
}

function isAuthority(value: string | undefined): value is GameAuthority {
  return value === "public" || value === "seat" || value === "spectator"
}

function HomeWorkspace() {
  return <WorkspaceConnect />
}

function GameRoute() {
  const { authority, gameId } = useParams()
  if (!gameId || !isAuthority(authority)) return <Navigate replace to="/" />
  const session: GameSession = {
    authority,
    gameId,
    serverOrigin: normalizeServerOrigin(DEFAULT_SERVER_ORIGIN),
  }
  return <LiveWorkspace session={session} />
}

const router = createBrowserRouter([
  { path: "/", element: <HomeWorkspace />, errorElement: <RouteError /> },
  {
    path: "/games/:gameId/:authority",
    element: <GameRoute />,
    errorElement: <RouteError />,
  },
  { path: "*", element: <Navigate replace to="/" /> },
])

export function App() {
  return (
    <AppErrorBoundary>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </AppErrorBoundary>
  )
}

function WorkspaceHeader({ subtitle }: { subtitle: string }) {
  const { setTheme, theme } = useTheme()
  const systemDark = window.matchMedia("(prefers-color-scheme: dark)").matches
  const dark = theme === "dark" || (theme === "system" && systemDark)
  return (
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
              {subtitle}
            </p>
          </div>
        </div>
        <Button
          aria-label={`Switch to ${dark ? "light" : "dark"} theme`}
          onClick={() => setTheme(dark ? "light" : "dark")}
          size="icon"
          variant="outline"
        >
          {dark ? <Sun /> : <Moon />}
        </Button>
      </div>
    </header>
  )
}

function WorkspaceConnect({
  onConnected,
  session,
}: {
  onConnected?: (token: string) => void
  session?: GameSession
}) {
  const navigate = useNavigate()
  const serverOrigin = normalizeServerOrigin(
    session?.serverOrigin ?? DEFAULT_SERVER_ORIGIN
  )
  const [gameId, setGameId] = useState(session?.gameId ?? "")
  const [authority, setAuthority] = useState<GameAuthority>(
    session?.authority ?? "spectator"
  )
  const [token, setToken] = useState("")
  const [error, setError] = useState<string>()

  const submit = (event: FormEvent) => {
    event.preventDefault()
    try {
      const next: GameSession = {
        authority,
        gameId: gameId.trim(),
        serverOrigin,
      }
      if (!next.gameId) throw new Error("Enter a game ID")
      credentialVault.set(next, token)
      queryClient.removeQueries({ queryKey: gameQueryKey(next) })
      if (session && onConnected) {
        onConnected(token.trim())
      } else {
        navigate(`/games/${encodeURIComponent(next.gameId)}/${next.authority}`)
      }
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Unable to connect")
    }
  }

  return (
    <div className="min-h-svh bg-background">
      <WorkspaceHeader subtitle="Connect to an active local game" />
      <main className="mx-auto grid max-w-[1200px] items-start gap-4 p-3 sm:p-5 lg:grid-cols-[minmax(0,1fr)_24rem]">
        <Card className="overflow-hidden" size="sm">
          <CardHeader className="border-b">
            <CardTitle>Game board</CardTitle>
            <CardDescription>
              The authoritative board appears after capability authentication.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-3" aria-hidden="true">
            <div className="grid aspect-square grid-cols-15 gap-px rounded-xl bg-board-line p-2 opacity-40">
              {placeholderSquares.map((square) => (
                <span className="rounded-[2px] bg-board" key={square} />
              ))}
            </div>
          </CardContent>
        </Card>
        <Card size="sm">
          <CardHeader className="border-b">
            <CardTitle>Open game workspace</CardTitle>
            <CardDescription>
              Credentials stay in this tab's memory and are never stored or
              added to the URL.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <form className="space-y-4" onSubmit={submit}>
              <div className="space-y-1.5">
                <Label htmlFor="server-origin">Referee server</Label>
                <Input id="server-origin" readOnly value={serverOrigin} />
                <p className="text-xs text-muted-foreground">
                  Configure another origin with VITE_WORD_ARENA_SERVER.
                </p>
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="game-id">Game ID</Label>
                <Input
                  autoComplete="off"
                  id="game-id"
                  onChange={(event) => setGameId(event.target.value)}
                  required
                  value={gameId}
                />
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="authority">View</Label>
                <Select
                  onValueChange={(value) =>
                    setAuthority(value as GameAuthority)
                  }
                  value={authority}
                >
                  <SelectTrigger className="w-full" id="authority">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="spectator">Human spectator</SelectItem>
                    <SelectItem value="seat">Private player seat</SelectItem>
                    <SelectItem value="public">Public board</SelectItem>
                  </SelectContent>
                </Select>
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="capability">Capability</Label>
                <Input
                  autoCapitalize="none"
                  autoComplete="off"
                  id="capability"
                  onChange={(event) => setToken(event.target.value)}
                  required
                  type="password"
                  value={token}
                />
              </div>
              {error ? (
                <Alert variant="destructive">
                  <AlertCircle />
                  <AlertTitle>Connection details rejected</AlertTitle>
                  <AlertDescription>{error}</AlertDescription>
                </Alert>
              ) : null}
              <Button className="w-full" type="submit">
                <Radio /> Open live game
              </Button>
            </form>
          </CardContent>
        </Card>
      </main>
    </div>
  )
}

function useLiveGame(session: GameSession, token: string) {
  const [connection, setConnection] = useState<ConnectionState>("connecting")
  const [view, setView] = useState<GameView>()
  const [error, setError] = useState<Error>()
  const version = useRef(0)

  useEffect(() => {
    let cancelled = false
    let disconnect: (() => void) | undefined
    const load = async () => {
      try {
        const next = await queryClient.fetchQuery(gameQueryOptions(session))
        if (cancelled) return undefined
        version.current = next.public.state.version
        setView(next)
        setError(undefined)
        return next
      } catch (caught) {
        if (cancelled) return undefined
        setError(
          caught instanceof Error ? caught : new Error("Snapshot failed")
        )
        setConnection("offline")
        return undefined
      }
    }
    void load().then((initial) => {
      if (!initial || cancelled) return
      disconnect = connectInvalidationSocket({
        session,
        token,
        getVersion: () => version.current,
        onStateChange: setConnection,
        onInvalidation: async () => {
          await queryClient.invalidateQueries({
            queryKey: gameQueryKey(session),
          })
          await load()
        },
      })
    })
    return () => {
      cancelled = true
      disconnect?.()
    }
  }, [session, token])

  return { connection, error, view }
}

function LiveWorkspace({ session }: { session: GameSession }) {
  const [token, setToken] = useState(() => credentialVault.get(session))
  if (!token) {
    return <WorkspaceConnect onConnected={setToken} session={session} />
  }
  return <AuthenticatedWorkspace session={session} token={token} />
}

function AuthenticatedWorkspace({
  session,
  token,
}: {
  session: GameSession
  token: string
}) {
  const { connection, error, view } = useLiveGame(session, token)
  if (error && !view) {
    return (
      <div className="min-h-svh bg-background">
        <WorkspaceHeader subtitle={`Game ${session.gameId}`} />
        <main className="mx-auto max-w-xl p-4 sm:p-8">
          <Alert variant="destructive">
            <Unplug />
            <AlertTitle>Unable to load this projection</AlertTitle>
            <AlertDescription>{error.message}</AlertDescription>
          </Alert>
          <Button className="mt-4" onClick={() => window.location.assign("/")}>
            Use another credential
          </Button>
        </main>
      </div>
    )
  }
  if (!view) return <LoadingWorkspace gameId={session.gameId} />
  return <GameWorkspace connection={connection} view={view} />
}

function LoadingWorkspace({ gameId }: { gameId: string }) {
  return (
    <div className="min-h-svh bg-background">
      <WorkspaceHeader subtitle={`Game ${gameId}`} />
      <main className="mx-auto grid max-w-[1200px] gap-4 p-4 lg:grid-cols-[1fr_18rem]">
        <Skeleton className="aspect-square rounded-xl" />
        <div className="space-y-4">
          <Skeleton className="h-36" />
          <Skeleton className="h-64" />
        </div>
      </main>
    </div>
  )
}

function GameWorkspace({
  connection,
  view,
}: {
  connection: ConnectionState
  view: GameView
}) {
  const state = view.public.state
  const tiles: Record<string, BoardTile> = {}
  state.board.forEach((tile, index) => {
    if (tile) {
      tiles[`${Math.floor(index / 15)}-${index % 15}`] = {
        letter: tile.letter,
      }
    }
  })
  const moves = toMoveRecords(view)
  return (
    <div className="min-h-svh bg-background">
      <WorkspaceHeader
        subtitle={`Game ${state.game_id} · ${view.authority} view`}
      />
      <main className="mx-auto grid max-w-[1600px] items-start gap-3 p-3 sm:p-5 lg:grid-cols-[minmax(0,1fr)_18rem] xl:grid-cols-[15rem_minmax(0,1fr)_18rem]">
        <aside className="grid gap-3 sm:grid-cols-2 lg:col-span-2 xl:col-span-1 xl:grid-cols-1">
          <PlayerCard
            active={state.phase === "active" && state.current_player === "one"}
            agent="Seat one"
            clock="--:--"
            score={state.scores[0]}
            subtitle={`${state.rack_counts[0]} tiles`}
          />
          <PlayerCard
            active={state.phase === "active" && state.current_player === "two"}
            agent="Seat two"
            clock="--:--"
            score={state.scores[1]}
            subtitle={`${state.rack_counts[1]} tiles`}
          />
          <Card className="sm:col-span-2 xl:col-span-1" size="sm">
            <CardHeader className="border-b">
              <CardTitle>Match</CardTitle>
              <CardDescription>Authoritative configuration</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3">
              <div className="flex justify-between gap-3">
                <span className="flex items-center gap-2 text-muted-foreground">
                  <Languages className="size-4" /> Rules
                </span>
                <span className="font-medium">{state.ruleset_id}</span>
              </div>
              <div className="flex justify-between gap-3">
                <span className="flex items-center gap-2 text-muted-foreground">
                  <Layers3 className="size-4" /> Bag
                </span>
                <span className="font-medium">{state.bag_count} tiles</span>
              </div>
              <Separator />
              <div className="flex justify-between gap-3 text-xs">
                <span className="text-muted-foreground">Mode</span>
                <Badge variant="outline">{state.mode}</Badge>
              </div>
            </CardContent>
          </Card>
        </aside>
        <section className="min-w-0 lg:col-start-1 xl:col-start-2">
          <Card className="gap-3" size="sm">
            <CardHeader className="border-b sm:grid-cols-[1fr_auto]">
              <div>
                <div className="mb-1 flex items-center gap-2">
                  <Badge>
                    {state.phase === "active"
                      ? `${state.current_player} to move`
                      : "Finished"}
                  </Badge>
                  <span className="text-xs text-muted-foreground">
                    Version {state.version}
                  </span>
                </div>
                <CardTitle>Live board</CardTitle>
                <CardDescription>
                  Server snapshots are the only authoritative state.
                </CardDescription>
              </div>
              <Badge variant={connection === "live" ? "secondary" : "outline"}>
                {connection}
              </Badge>
            </CardHeader>
            <CardContent>
              <GameBoard tiles={tiles} />
            </CardContent>
          </Card>
        </section>
        <aside className="min-w-0 lg:col-start-2 lg:row-start-2 xl:col-start-3 xl:row-start-1">
          <MoveHistory moves={moves} />
          <Card className="mt-3" size="sm">
            <CardHeader className="border-b">
              <CardTitle>Projection boundary</CardTitle>
              <CardDescription>{view.authority} capability</CardDescription>
            </CardHeader>
            <CardContent className="text-xs leading-5 text-muted-foreground">
              {view.authority === "seat"
                ? `Only seat ${view.seat}'s rack is available.`
                : view.authority === "spectator"
                  ? "Both current racks are available; the future bag and seed remain hidden."
                  : "Only public board and history data are available."}
            </CardContent>
          </Card>
        </aside>
      </main>
    </div>
  )
}

function toMoveRecords(view: GameView): MoveRecord[] {
  return view.public.events
    .filter((event) => event.kind.type !== "created")
    .slice(-12)
    .reverse()
    .map((event) => {
      const player = event.kind.player === "two" ? "Seat two" : "Seat one"
      const score = typeof event.kind.score === "number" ? event.kind.score : 0
      const words = Array.isArray(event.kind.words) ? event.kind.words : []
      const first = words[0]
      const word =
        typeof first === "object" && first !== null && "text" in first
          ? String(first.text)
          : event.kind.type.replaceAll("_", " ").toUpperCase()
      return {
        player,
        turn: event.sequence,
        word,
        score,
        detail: event.kind.type.replaceAll("_", " "),
        elapsed: "authoritative",
      }
    })
}

export default App
