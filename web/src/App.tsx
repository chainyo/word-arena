import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import {
  AlertCircle,
  Bot,
  Eye,
  History,
  Languages,
  Layers3,
  LoaderCircle,
  Minus,
  Monitor,
  Moon,
  Plus,
  Radio,
  RefreshCw,
  Sun,
  Trophy,
  Unplug,
  UserRound,
} from "lucide-react"
import {
  Component,
  type ErrorInfo,
  type FormEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
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

import {
  createAgentMatch,
  DEFAULT_SERVER_ORIGIN,
  fetchAgentCatalog,
  fetchAgentMatchActivity,
  fetchAgentMatches,
  fetchAgentMatchStatus,
  fetchSpectatorReplay,
  normalizeServerOrigin,
  recoverAgentMatch,
  submitGameAction,
} from "@/api/client"
import { credentialVault } from "@/api/credentials"
import { gameQueryKey, gameQueryOptions, rulesQueryOptions } from "@/api/query"
import { classifySessionFailure, connectionMessage } from "@/api/session"
import type {
  AgentCatalogEntry,
  AgentHarnessId,
  AgentMatchActivity,
  AgentMatchList,
  AgentMatchStatus,
  AgentSeatSelection,
  AgentSeatStatus,
  ConnectionState,
  Coordinate,
  GameAuthority,
  GameMove,
  GameSession,
  GameView,
  Ruleset,
} from "@/api/types"
import { connectInvalidationSocket } from "@/api/websocket"
import { AgentLogo } from "@/components/agent/agent-logo"
import { AgentConsole } from "@/components/game/agent-console"
import { BlankAssignmentDialog } from "@/components/game/blank-assignment-dialog"
import {
  displayLetterValues,
  displayPremiums,
} from "@/components/game/display-rules"
import { type BoardTile, GameBoard } from "@/components/game/game-board"
import { GameClock } from "@/components/game/game-clock"
import { GameControls } from "@/components/game/game-controls"
import { GameRack, type RackTile } from "@/components/game/game-rack"
import {
  EMPTY_MOVE_DRAFT,
  type MoveDraft,
  physicalLetter,
  removePlacement,
  selectRackTile,
  setDraftMode,
  stageSelectedTile,
} from "@/components/game/move-draft"
import {
  MoveHistory,
  type MoveRecord,
  moveSummary,
} from "@/components/game/move-history"
import { tileOwnersFromEvents } from "@/components/game/move-ownership"
import { PlayerCard } from "@/components/game/player-card"
import { ReplayView } from "@/components/replay/replay-view"
import { type Theme, useTheme } from "@/components/theme-provider"
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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table"
import {
  languageLabel,
  rulesetLabel,
  SEATS,
  seatLabel,
} from "@/lib/game-labels"

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
    <main
      id="main-content"
      tabIndex={-1}
      className="grid min-h-svh place-items-center bg-background p-4"
    >
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
  return <OperatorWorkspace />
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

function AuthorityGameRoute({ authority }: { authority: GameAuthority }) {
  const { gameId } = useParams()
  if (!gameId) return <Navigate replace to="/" />
  return (
    <LiveWorkspace
      session={{
        authority,
        gameId,
        serverOrigin: normalizeServerOrigin(DEFAULT_SERVER_ORIGIN),
      }}
    />
  )
}

const router = createBrowserRouter([
  { path: "/", element: <HomeWorkspace />, errorElement: <RouteError /> },
  {
    path: "/operator",
    element: <HomeWorkspace />,
    errorElement: <RouteError />,
  },
  {
    path: "/connect",
    element: <WorkspaceConnect />,
    errorElement: <RouteError />,
  },
  {
    path: "/games/:gameId/player",
    element: <AuthorityGameRoute authority="seat" />,
    errorElement: <RouteError />,
  },
  {
    path: "/games/:gameId/spectator",
    element: <AuthorityGameRoute authority="spectator" />,
    errorElement: <RouteError />,
  },
  {
    path: "/games/:gameId/public",
    element: <AuthorityGameRoute authority="public" />,
    errorElement: <RouteError />,
  },
  {
    path: "/games/:gameId/replay",
    element: <ReplayRoute />,
    errorElement: <RouteError />,
  },
  {
    path: "/tournaments",
    element: <HomeWorkspace />,
    errorElement: <RouteError />,
  },
  {
    path: "/tournaments/:tournamentId/standings",
    element: <DeferredDataRoute kind="standings" />,
    errorElement: <RouteError />,
  },
  {
    path: "/agents/:agentId",
    element: <DeferredDataRoute kind="agent" />,
    errorElement: <RouteError />,
  },
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

function OperatorWorkspace() {
  const navigate = useNavigate()
  const serverOrigin = normalizeServerOrigin(DEFAULT_SERVER_ORIGIN)
  const [language, setLanguage] = useState<"english" | "french">("english")
  const [mode, setMode] = useState<"competitive" | "practice">("competitive")
  const [catalog, setCatalog] = useState<AgentCatalogEntry[]>([])
  const [catalogPending, setCatalogPending] = useState(true)
  const [seats, setSeats] = useState<SeatDraft[]>([
    { kind: "agent", harness: "codex", model: "" },
    { kind: "agent", harness: "codex", model: "" },
  ])
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<string>()
  const [matches, setMatches] = useState<AgentMatchList>({ matches: [] })
  const [matchesError, setMatchesError] = useState<string>()
  const [openingGameId, setOpeningGameId] = useState<string>()

  useEffect(() => {
    const controller = new AbortController()
    setCatalogPending(true)
    void fetchAgentCatalog(serverOrigin, controller.signal)
      .then((entries) => {
        setCatalog(entries)
        const first = entries.find(
          (entry) => entry.available && entry.compatible
        )
        if (first) {
          setSeats((current) =>
            current.map((seat) =>
              seat.kind === "agent" &&
              !entries.some(
                (entry) =>
                  entry.id === seat.harness &&
                  entry.available &&
                  entry.compatible
              )
                ? { ...seat, harness: first.id }
                : seat
            )
          )
        }
      })
      .catch((caught) => {
        if (!controller.signal.aborted) {
          setError(
            caught instanceof Error
              ? caught.message
              : "Unable to inspect local agents"
          )
        }
      })
      .finally(() => {
        if (!controller.signal.aborted) setCatalogPending(false)
      })
    return () => controller.abort()
  }, [serverOrigin])

  useEffect(() => {
    let cancelled = false
    const controller = new AbortController()
    const load = async () => {
      try {
        const next = await fetchAgentMatches(serverOrigin, controller.signal)
        if (!cancelled) {
          setMatches(next)
          setMatchesError(undefined)
        }
      } catch (caught) {
        if (!cancelled && !controller.signal.aborted) {
          setMatchesError(
            caught instanceof Error
              ? caught.message
              : "Unable to load local matches"
          )
        }
      }
    }
    void load()
    const timer = window.setInterval(() => void load(), 2_000)
    return () => {
      cancelled = true
      controller.abort()
      window.clearInterval(timer)
    }
  }, [serverOrigin])

  const openMatch = async (match: AgentMatchStatus) => {
    setOpeningGameId(match.gameId)
    setMatchesError(undefined)
    try {
      const recovered = await recoverAgentMatch(serverOrigin, match.gameId)
      const session: GameSession = {
        authority: "spectator",
        gameId: match.gameId,
        serverOrigin,
      }
      credentialVault.set(session, recovered.spectatorCapability)
      navigate(
        match.phase === "finished"
          ? `/games/${encodeURIComponent(match.gameId)}/replay`
          : `/games/${encodeURIComponent(match.gameId)}/spectator`
      )
    } catch (caught) {
      setMatchesError(
        caught instanceof Error ? caught.message : "Unable to open this match"
      )
    } finally {
      setOpeningGameId(undefined)
    }
  }

  const createGame = async (event: FormEvent) => {
    event.preventDefault()
    setPending(true)
    setError(undefined)
    try {
      const selected = seats.map(toSeatSelection)
      const created = await createAgentMatch(serverOrigin, {
        language,
        mode,
        seats: selected,
        idempotency_key: `web-match-${crypto.randomUUID()}`,
      })
      const publicSession: GameSession = {
        authority: "public",
        gameId: created.gameId,
        serverOrigin,
      }
      const spectatorSession: GameSession = {
        ...publicSession,
        authority: "spectator",
      }
      credentialVault.set(publicSession, created.publicCapability)
      credentialVault.set(spectatorSession, created.spectatorCapability)
      queryClient.setQueryData(gameQueryKey(publicSession), {
        authority: "public",
        observedAt: Date.now(),
        public: created.public,
      } satisfies GameView)
      if (created.humanCapability) {
        const humanSession: GameSession = {
          ...publicSession,
          authority: "seat",
        }
        credentialVault.set(humanSession, created.humanCapability)
        navigate(`/games/${encodeURIComponent(created.gameId)}/player`)
      } else {
        navigate(`/games/${encodeURIComponent(created.gameId)}/spectator`)
      }
    } catch (caught) {
      setError(
        caught instanceof Error ? caught.message : "Unable to create this game"
      )
    } finally {
      setPending(false)
    }
  }

  return (
    <div className="min-h-svh bg-background">
      <WorkspaceHeader subtitle="Agent match console" />
      <main
        id="main-content"
        tabIndex={-1}
        className="mx-auto max-w-6xl space-y-6 p-3 sm:p-6"
      >
        <form
          className="space-y-4"
          onSubmit={(event) => void createGame(event)}
        >
          <div className="flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
            <div>
              <Badge variant="secondary">
                <Bot /> Agent-first
              </Badge>
              <h2 className="mt-3 font-heading text-2xl font-semibold tracking-tight">
                Start a match
              </h2>
              <p className="mt-1 text-sm text-muted-foreground">
                Start with two players, or add up to four. Humans are optional;
                agents play through MCP.
              </p>
            </div>
            <Button
              onClick={() => navigate("/connect")}
              type="button"
              variant="ghost"
            >
              <Radio /> Open existing game
            </Button>
          </div>

          <div className="grid gap-4 lg:grid-cols-2">
            {seats.map((seat, index) => (
              <AgentSeatPicker
                catalog={catalog}
                disabled={pending || catalogPending}
                key={SEATS[index]}
                label={`Seat ${index + 1}`}
                onChange={(next) =>
                  setSeats((current) => {
                    const updated = [...current]
                    updated[index] = next
                    return updated
                  })
                }
                onRemove={
                  seats.length > 2 && index >= 2
                    ? () =>
                        setSeats((current) =>
                          current.filter(
                            (_, currentIndex) => currentIndex !== index
                          )
                        )
                    : undefined
                }
                seat={seat}
                humanAllowed={seats.every(
                  (other, otherIndex) =>
                    otherIndex === index || other.kind === "agent"
                )}
              />
            ))}
          </div>

          {seats.length < 4 ? (
            <Button
              disabled={pending || catalogPending}
              onClick={() =>
                setSeats((current) => [
                  ...current,
                  { kind: "agent", harness: "codex", model: "" },
                ])
              }
              type="button"
              variant="outline"
            >
              <Plus /> Add player
            </Button>
          ) : null}

          <Card size="sm">
            <CardContent className="grid gap-4 pt-4 sm:grid-cols-[1fr_1fr_auto] sm:items-end">
              <div className="space-y-1.5">
                <Label htmlFor="create-language">Language</Label>
                <Select
                  onValueChange={(value) =>
                    setLanguage(value as "english" | "french")
                  }
                  value={language}
                >
                  <SelectTrigger className="w-full" id="create-language">
                    <SelectValue>{languageLabel(language)}</SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="english">🇬🇧 English</SelectItem>
                    <SelectItem value="french">🇫🇷 Français</SelectItem>
                  </SelectContent>
                </Select>
              </div>
              <div className="space-y-1.5">
                <Label htmlFor="create-mode">Rules</Label>
                <Select
                  onValueChange={(value) =>
                    setMode(value as "competitive" | "practice")
                  }
                  value={mode}
                >
                  <SelectTrigger className="w-full" id="create-mode">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="competitive">Competitive</SelectItem>
                    <SelectItem value="practice">Practice</SelectItem>
                  </SelectContent>
                </Select>
              </div>
              <Button
                className="min-w-40"
                disabled={
                  pending ||
                  catalogPending ||
                  catalog.every((agent) => !agent.compatible)
                }
                size="lg"
                type="submit"
              >
                {pending ? (
                  <LoaderCircle className="animate-spin motion-reduce:animate-none" />
                ) : (
                  <Plus />
                )}
                Start match
              </Button>
            </CardContent>
          </Card>

          {error ? (
            <Alert variant="destructive">
              <AlertCircle />
              <AlertTitle>Match could not start</AlertTitle>
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          ) : null}
          <p className="text-center text-xs text-muted-foreground">
            {catalogPending
              ? "Checking installed agents…"
              : `${catalog.filter((agent) => agent.available && agent.compatible).length} of ${catalog.length} compatible CLIs · ${serverOrigin}`}
          </p>
        </form>
        <MatchArchive
          error={matchesError}
          matches={matches.matches}
          onOpen={(match) => void openMatch(match)}
          openingGameId={openingGameId}
        />
      </main>
    </div>
  )
}

function MatchArchive({
  error,
  matches,
  onOpen,
  openingGameId,
}: {
  error?: string
  matches: AgentMatchStatus[]
  onOpen: (match: AgentMatchStatus) => void
  openingGameId?: string
}) {
  const live = matches.filter(
    (match) => match.phase === "active" && match.orchestration === "active"
  )
  const history = matches.filter(
    (match) => match.phase === "finished" || match.orchestration !== "active"
  )
  return (
    <section aria-labelledby="local-matches-title" className="space-y-4">
      <div className="flex items-end justify-between gap-3">
        <div>
          <h2
            className="font-heading text-xl font-semibold tracking-tight"
            id="local-matches-title"
          >
            Local matches
          </h2>
          <p className="mt-1 text-sm text-muted-foreground">
            Reopen a live board or inspect a completed replay after refreshing.
          </p>
        </div>
        <Badge variant="outline">{matches.length} retained</Badge>
      </div>
      {error ? (
        <Alert variant="destructive">
          <AlertCircle />
          <AlertTitle>Match archive unavailable</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}
      <MatchArchiveCard
        description="Currently driven by the local agent runner"
        empty="No live matches"
        matches={live}
        onOpen={onOpen}
        openingGameId={openingGameId}
        title="Live"
      />
      <MatchArchiveCard
        description="Finished games and runners interrupted by a server restart"
        empty="No completed matches yet"
        matches={history}
        onOpen={onOpen}
        openingGameId={openingGameId}
        title="History"
      />
    </section>
  )
}

function MatchArchiveCard({
  description,
  empty,
  matches,
  onOpen,
  openingGameId,
  title,
}: {
  description: string
  empty: string
  matches: AgentMatchStatus[]
  onOpen: (match: AgentMatchStatus) => void
  openingGameId?: string
  title: string
}) {
  return (
    <Card className="overflow-hidden" size="sm">
      <CardHeader className="border-b">
        <CardTitle aria-level={3} role="heading">
          {title}
        </CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      {matches.length === 0 ? (
        <CardContent className="py-8 text-center text-sm text-muted-foreground">
          {empty}
        </CardContent>
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Players</TableHead>
              <TableHead>Game</TableHead>
              <TableHead>Score</TableHead>
              <TableHead>Updated</TableHead>
              <TableHead className="text-right">Action</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {matches.map((match) => (
              <TableRow key={match.gameId}>
                <TableCell>
                  <div className="flex items-center gap-1.5">
                    {match.seats.map((seat) =>
                      seat.participant.kind === "agent" ? (
                        <AgentLogo
                          agent={seat.participant.harness}
                          className="size-7 rounded-lg [&_svg]:size-4"
                          key={seat.seat}
                        />
                      ) : (
                        <span
                          className="grid size-7 place-items-center rounded-lg border bg-background"
                          key={seat.seat}
                        >
                          <UserRound className="size-4" />
                        </span>
                      )
                    )}
                    <span className="ml-1 max-w-48 truncate font-medium">
                      {match.seats.map(participantLabel).join(" vs ")}
                    </span>
                  </div>
                </TableCell>
                <TableCell>
                  <div className="flex items-center gap-2">
                    <Badge
                      variant={
                        match.orchestration === "active"
                          ? "secondary"
                          : "outline"
                      }
                    >
                      {match.orchestration}
                    </Badge>
                    <span className="text-muted-foreground capitalize">
                      {languageLabel(match.language)} · {match.mode} · turn{" "}
                      {match.version}
                    </span>
                  </div>
                  <div className="mt-1 max-w-72 truncate font-mono text-[11px] text-muted-foreground">
                    {match.gameId}
                  </div>
                </TableCell>
                <TableCell className="font-heading text-base font-semibold tabular-nums">
                  {match.scores.join("–")}
                </TableCell>
                <TableCell className="text-muted-foreground">
                  {formatMatchTime(match.updatedAtUnixMs)}
                </TableCell>
                <TableCell className="text-right">
                  <Button
                    aria-label={`Open ${match.gameId}`}
                    disabled={openingGameId === match.gameId}
                    onClick={() => onOpen(match)}
                    size="sm"
                    variant="outline"
                  >
                    {openingGameId === match.gameId ? (
                      <LoaderCircle className="animate-spin motion-reduce:animate-none" />
                    ) : match.phase === "finished" ? (
                      <History />
                    ) : (
                      <Eye />
                    )}
                    {match.phase === "finished" ? "Replay" : "Watch"}
                  </Button>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
    </Card>
  )
}

function participantLabel(status: AgentSeatStatus) {
  if (status.participant.kind === "human") return status.participant.name
  if (status.participant.harness === "claude_code") return "Claude Code"
  if (status.participant.harness === "codex") return "Codex"
  if (status.participant.harness === "cline") return "Cline"
  return "Pi"
}

function formatMatchTime(value: number) {
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    month: "short",
    day: "numeric",
  }).format(new Date(value))
}

type SeatDraft =
  | { kind: "agent"; harness: AgentHarnessId; model: string }
  | { kind: "human"; name: string }

function toSeatSelection(seat: SeatDraft): AgentSeatSelection {
  if (seat.kind === "human") return seat
  const model = seat.model.trim()
  return {
    kind: "agent",
    harness: seat.harness,
    model: model || undefined,
  }
}

function AgentSeatPicker({
  catalog,
  disabled,
  humanAllowed,
  label,
  onChange,
  onRemove,
  seat,
}: {
  catalog: AgentCatalogEntry[]
  disabled: boolean
  humanAllowed: boolean
  label: string
  onChange: (seat: SeatDraft) => void
  onRemove?: () => void
  seat: SeatDraft
}) {
  return (
    <Card size="sm">
      <CardHeader className="border-b">
        <div className="flex items-center justify-between gap-3">
          <div>
            <CardTitle>{label}</CardTitle>
            <CardDescription>
              {seat.kind === "agent"
                ? "Autonomous MCP player"
                : "Local human player"}
            </CardDescription>
          </div>
          <div className="flex items-center gap-2">
            {onRemove ? (
              <Button
                aria-label={`Remove ${label}`}
                disabled={disabled}
                onClick={onRemove}
                size="icon-sm"
                type="button"
                variant="ghost"
              >
                <Minus />
              </Button>
            ) : null}
            <Button
              disabled={disabled || (!humanAllowed && seat.kind === "agent")}
              onClick={() =>
                onChange(
                  seat.kind === "agent"
                    ? { kind: "human", name: "Human" }
                    : { kind: "agent", harness: "codex", model: "" }
                )
              }
              size="sm"
              type="button"
              variant="outline"
            >
              {seat.kind === "agent" ? <UserRound /> : <Bot />}
              {seat.kind === "agent" ? "Use human" : "Use agent"}
            </Button>
          </div>
        </div>
      </CardHeader>
      <CardContent className="space-y-4 pt-4">
        {seat.kind === "human" ? (
          <div className="space-y-1.5">
            <Label htmlFor={`${label}-human-name`}>Player name</Label>
            <Input
              id={`${label}-human-name`}
              maxLength={64}
              onChange={(event) =>
                onChange({ kind: "human", name: event.target.value })
              }
              value={seat.name}
            />
          </div>
        ) : (
          <>
            <div
              className="grid grid-cols-2 gap-2"
              role="radiogroup"
              aria-label={`${label} agent`}
            >
              {catalog.map((agent) => {
                const ready = agent.available && agent.compatible
                const selected = seat.harness === agent.id
                return (
                  <label
                    className={`relative flex min-h-20 items-center gap-3 rounded-xl border p-3 text-left transition-colors has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring ${selected ? "border-primary bg-accent" : "bg-card hover:bg-accent/60"} ${ready ? "" : "cursor-not-allowed opacity-45"}`}
                    key={agent.id}
                  >
                    <input
                      checked={selected}
                      className="absolute inset-0 z-10 cursor-pointer opacity-0 disabled:cursor-not-allowed"
                      disabled={disabled || !ready}
                      name={`${label}-agent`}
                      onChange={() => onChange({ ...seat, harness: agent.id })}
                      type="radio"
                      value={agent.id}
                    />
                    <AgentLogo agent={agent.id} />
                    <span className="min-w-0">
                      <span className="block truncate text-sm font-medium">
                        {agent.displayName}
                      </span>
                      <span
                        className={`block truncate text-xs ${selected ? "text-foreground" : "text-muted-foreground"}`}
                      >
                        {agent.version ? `v${agent.version}` : agent.diagnostic}
                      </span>
                    </span>
                  </label>
                )
              })}
            </div>
            <div className="space-y-1.5">
              <Label htmlFor={`${label}-model`}>Model override</Label>
              <Input
                id={`${label}-model`}
                maxLength={128}
                onChange={(event) =>
                  onChange({ ...seat, model: event.target.value })
                }
                placeholder="Use agent default"
                value={seat.model}
              />
            </div>
          </>
        )}
      </CardContent>
    </Card>
  )
}

function ReplayRoute() {
  const { gameId } = useParams()
  const session = useMemo<GameSession | undefined>(
    () =>
      gameId
        ? {
            authority: "spectator",
            gameId,
            serverOrigin: normalizeServerOrigin(DEFAULT_SERVER_ORIGIN),
          }
        : undefined,
    [gameId]
  )
  const { recovering, recoveryFailed, setToken, token } =
    useLocalSpectatorCredential(session)
  const [replay, setReplay] =
    useState<Awaited<ReturnType<typeof fetchSpectatorReplay>>>()
  const [error, setError] = useState<string>()
  useEffect(() => {
    if (!session || !token) return undefined
    const controller = new AbortController()
    setError(undefined)
    void fetchSpectatorReplay(session, token, controller.signal)
      .then(setReplay)
      .catch((caught) => {
        if (!controller.signal.aborted) {
          setError(caught instanceof Error ? caught.message : "Replay failed")
        }
      })
    return () => controller.abort()
  }, [session, token])
  if (!session) return <Navigate replace to="/" />
  if (recovering) return <LoadingWorkspace gameId={session.gameId} />
  if (!token) {
    return recoveryFailed ? (
      <WorkspaceConnect onConnected={setToken} session={session} />
    ) : (
      <LoadingWorkspace gameId={session.gameId} />
    )
  }
  return (
    <div className="min-h-svh bg-background">
      <WorkspaceHeader
        subtitle={`Game ${session.gameId} · human-spectator replay`}
      />
      {error ? (
        <main
          id="main-content"
          tabIndex={-1}
          className="mx-auto max-w-xl p-4 sm:p-8"
        >
          <Alert variant="destructive">
            <AlertCircle />
            <AlertTitle>Replay unavailable</AlertTitle>
            <AlertDescription>{error}</AlertDescription>
          </Alert>
          <Button
            className="mt-4"
            onClick={() => {
              credentialVault.delete(session)
              setToken(undefined)
            }}
            variant="outline"
          >
            Use another spectator capability
          </Button>
        </main>
      ) : replay ? (
        <ReplayView
          gameId={session.gameId}
          onShare={() =>
            void navigator.clipboard?.writeText(
              `${window.location.origin}/games/${encodeURIComponent(session.gameId)}/public`
            )
          }
          replay={replay}
        />
      ) : (
        <main
          id="main-content"
          tabIndex={-1}
          className="mx-auto grid max-w-[1200px] gap-4 p-4 lg:grid-cols-[1fr_18rem]"
        >
          <Skeleton className="aspect-square rounded-xl" />
          <div className="space-y-4">
            <Skeleton className="h-36" />
            <Skeleton className="h-64" />
          </div>
        </main>
      )}
    </div>
  )
}

function DeferredDataRoute({ kind }: { kind: "agent" | "standings" }) {
  const params = useParams()
  const identifier = kind === "agent" ? params.agentId : params.tournamentId
  const title = kind === "agent" ? "Agent detail" : "Tournament standings"
  return (
    <div className="min-h-svh bg-background">
      <WorkspaceHeader subtitle={`${title} · public aggregate authority`} />
      <main
        id="main-content"
        tabIndex={-1}
        className="mx-auto max-w-5xl p-3 sm:p-5"
      >
        <Card size="sm">
          <CardHeader className="border-b">
            <div className="mb-1 flex items-center gap-2">
              <Badge variant="outline">Public aggregate</Badge>
              <span className="font-mono text-xs text-muted-foreground">
                {identifier}
              </span>
            </div>
            <CardTitle>{title}</CardTitle>
            <CardDescription>
              This route accepts only policy-approved aggregate data; it never
              receives racks, transcripts, or capabilities.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="grid gap-2 sm:grid-cols-[1fr_13rem]">
              <Input
                aria-label={`Filter ${title}`}
                placeholder={`Filter ${title.toLowerCase()}`}
              />
              <Select defaultValue="all">
                <SelectTrigger aria-label="Language filter">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">All languages</SelectItem>
                  <SelectItem value="english">🇬🇧 English</SelectItem>
                  <SelectItem value="french">🇫🇷 Français</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="grid min-h-64 place-items-center rounded-xl border border-dashed p-6 text-center">
              <div>
                {kind === "agent" ? (
                  <Bot className="mx-auto mb-3 size-8 text-muted-foreground" />
                ) : (
                  <Trophy className="mx-auto mb-3 size-8 text-muted-foreground" />
                )}
                <p className="font-heading font-medium">
                  No aggregate record yet
                </p>
                <p className="mt-1 max-w-lg text-sm text-muted-foreground">
                  Tournament and statistics repositories arrive in Phase 6.
                  Until then this is an explicit empty state, not sample data.
                </p>
              </div>
            </div>
            <div className="flex items-center justify-between gap-3 text-xs text-muted-foreground">
              <span>0 records</span>
              <div className="flex gap-1">
                <Button disabled size="sm" variant="outline">
                  Previous
                </Button>
                <Button disabled size="sm" variant="outline">
                  Next
                </Button>
              </div>
            </div>
          </CardContent>
        </Card>
      </main>
    </div>
  )
}

function WorkspaceHeader({ subtitle }: { subtitle: string }) {
  const { resolvedTheme, setTheme, theme } = useTheme()
  return (
    <>
      <a
        className="fixed top-2 left-2 z-50 -translate-y-20 rounded-lg bg-background px-3 py-2 text-sm font-medium shadow-lg ring-2 ring-ring transition-transform focus:translate-y-0 motion-reduce:transition-none"
        href="#main-content"
      >
        Skip to game content
      </a>
      <header className="sticky top-0 z-20 border-b bg-background/95 backdrop-blur-sm">
        <div className="mx-auto flex min-h-14 max-w-[1600px] items-center justify-between gap-3 px-3 py-2 sm:px-5">
          <div className="flex min-w-0 items-center gap-3">
            <span className="grid size-8 shrink-0 place-items-center rounded-lg bg-primary font-heading text-xs font-semibold text-primary-foreground shadow-sm">
              WA
            </span>
            <div className="min-w-0">
              <h1 className="truncate font-heading text-sm font-medium tracking-tight">
                Word Arena
              </h1>
              <p className="truncate text-[11px] text-muted-foreground">
                {subtitle}
              </p>
            </div>
          </div>
          <nav aria-label="Workspace controls">
            <Select
              onValueChange={(value) => {
                if (value) setTheme(value as Theme)
              }}
              value={theme}
            >
              <SelectTrigger
                aria-label={`Color theme: ${theme}`}
                className="w-auto min-w-24"
                size="sm"
              >
                {resolvedTheme === "dark" ? <Moon /> : <Sun />}
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="end">
                <SelectItem value="light">
                  <Sun /> Light
                </SelectItem>
                <SelectItem value="dark">
                  <Moon /> Dark
                </SelectItem>
                <SelectItem value="system">
                  <Monitor /> System
                </SelectItem>
              </SelectContent>
            </Select>
          </nav>
        </div>
      </header>
    </>
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
      <main
        id="main-content"
        tabIndex={-1}
        className="mx-auto grid max-w-[1200px] items-start gap-4 p-3 sm:p-5 lg:grid-cols-[minmax(0,1fr)_24rem]"
      >
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
  const [rules, setRules] = useState<Ruleset>()
  const version = useRef(0)

  const load = useCallback(async () => {
    try {
      const next = await queryClient.fetchQuery(gameQueryOptions(session))
      version.current = next.public.state.version
      setView(next)
      setError(undefined)
      return next
    } catch (caught) {
      setError(caught instanceof Error ? caught : new Error("Snapshot failed"))
      return undefined
    }
  }, [session])

  const reload = useCallback(async () => {
    await queryClient.invalidateQueries({ queryKey: gameQueryKey(session) })
    return load()
  }, [load, session])

  useEffect(() => {
    let cancelled = false
    let disconnect: (() => void) | undefined
    const loadRules = async () => {
      try {
        const next = await queryClient.fetchQuery(rulesQueryOptions(session))
        if (!cancelled) setRules(next)
      } catch {
        // The immutable built-in display rules remain available if this
        // capability was not issued the public-rules scope.
      }
    }
    void loadRules()
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
  }, [load, session, token])

  const acceptAuthoritativeView = (next: GameView) => {
    version.current = next.public.state.version
    setView(next)
    setError(undefined)
    queryClient.setQueryData(gameQueryKey(session), next)
  }

  return { acceptAuthoritativeView, connection, error, reload, rules, view }
}

function LiveWorkspace({ session }: { session: GameSession }) {
  const { recovering, recoveryFailed, setToken, token } =
    useLocalSpectatorCredential(session)
  if (recovering) return <LoadingWorkspace gameId={session.gameId} />
  if (!token) {
    return recoveryFailed || session.authority !== "spectator" ? (
      <WorkspaceConnect onConnected={setToken} session={session} />
    ) : (
      <LoadingWorkspace gameId={session.gameId} />
    )
  }
  return (
    <AuthenticatedWorkspace
      onCredentialReset={() => {
        credentialVault.delete(session)
        queryClient.removeQueries({ queryKey: gameQueryKey(session) })
        setToken(undefined)
      }}
      session={session}
      token={token}
    />
  )
}

function useLocalSpectatorCredential(session?: GameSession) {
  const authority = session?.authority
  const gameId = session?.gameId
  const serverOrigin = session?.serverOrigin
  const [token, setTokenState] = useState(() =>
    session ? credentialVault.get(session) : undefined
  )
  const [recovery, setRecovery] = useState<"idle" | "recovering" | "failed">(
    "idle"
  )
  useEffect(() => {
    if (authority !== "spectator" || !gameId || !serverOrigin || token) {
      return undefined
    }
    const recoverySession: GameSession = {
      authority,
      gameId,
      serverOrigin,
    }
    const controller = new AbortController()
    setRecovery("recovering")
    void recoverAgentMatch(serverOrigin, gameId, controller.signal)
      .then((recovered) => {
        credentialVault.set(recoverySession, recovered.spectatorCapability)
        setTokenState(recovered.spectatorCapability)
        setRecovery("idle")
      })
      .catch(() => {
        if (!controller.signal.aborted) setRecovery("failed")
      })
    return () => controller.abort()
  }, [authority, gameId, serverOrigin, token])
  const setToken = (next?: string) => {
    setTokenState(next)
    if (next) setRecovery("idle")
  }
  return {
    recovering: recovery === "recovering",
    recoveryFailed: recovery === "failed",
    setToken,
    token,
  }
}

function AuthenticatedWorkspace({
  onCredentialReset,
  session,
  token,
}: {
  onCredentialReset: () => void
  session: GameSession
  token: string
}) {
  const { acceptAuthoritativeView, connection, error, reload, rules, view } =
    useLiveGame(session, token)
  const initialFailure = error ? classifySessionFailure(error) : undefined
  if (error && !view) {
    return (
      <div className="min-h-svh bg-background">
        <WorkspaceHeader subtitle={`Game ${session.gameId}`} />
        <main
          id="main-content"
          tabIndex={-1}
          className="mx-auto max-w-xl p-4 sm:p-8"
        >
          <Alert variant="destructive">
            <Unplug />
            <AlertTitle>
              {initialFailure === "credential"
                ? "Capability expired or revoked"
                : "Unable to load this projection"}
            </AlertTitle>
            <AlertDescription>{error.message}</AlertDescription>
          </Alert>
          <div className="mt-4 flex flex-wrap gap-2">
            {initialFailure === "credential" ? null : (
              <Button onClick={() => void reload()} variant="outline">
                <RefreshCw /> Retry snapshot
              </Button>
            )}
            <Button onClick={onCredentialReset}>Use another credential</Button>
          </div>
        </main>
      </div>
    )
  }
  if (!view) return <LoadingWorkspace gameId={session.gameId} />
  return (
    <GameWorkspace
      connection={connection}
      key={`${view.public.state.game_id}-${view.public.state.version}`}
      onAuthoritativeView={acceptAuthoritativeView}
      onCredentialReset={onCredentialReset}
      onReload={reload}
      rules={rules}
      session={session}
      token={token}
      view={view}
      snapshotError={error}
    />
  )
}

function LoadingWorkspace({ gameId }: { gameId: string }) {
  return (
    <div className="min-h-svh bg-background">
      <WorkspaceHeader subtitle={`Game ${gameId}`} />
      <main
        id="main-content"
        tabIndex={-1}
        className="mx-auto grid max-w-[1200px] gap-4 p-4 lg:grid-cols-[1fr_18rem]"
      >
        <Skeleton className="aspect-square rounded-xl" />
        <div className="space-y-4">
          <Skeleton className="h-36" />
          <Skeleton className="h-64" />
        </div>
      </main>
    </div>
  )
}

function useAgentMatchStatus(session: GameSession, token: string) {
  const [status, setStatus] = useState<AgentMatchStatus>()
  useEffect(() => {
    let cancelled = false
    const controller = new AbortController()
    const load = async () => {
      try {
        const next = await fetchAgentMatchStatus(
          session,
          token,
          controller.signal
        )
        if (!cancelled) setStatus(next)
      } catch {
        // Manually created games intentionally have no agent-match record.
      }
    }
    void load()
    const timer = window.setInterval(() => void load(), 1000)
    return () => {
      cancelled = true
      controller.abort()
      window.clearInterval(timer)
    }
  }, [session, token])
  return status
}

function useAgentMatchActivity(session: GameSession, token: string) {
  const [activity, setActivity] = useState<AgentMatchActivity>()
  useEffect(() => {
    if (session.authority !== "spectator") return
    let cancelled = false
    const controller = new AbortController()
    const load = async () => {
      try {
        const next = await fetchAgentMatchActivity(
          session,
          token,
          controller.signal
        )
        if (!cancelled) setActivity(next)
      } catch {
        // Manually created games intentionally have no agent activity record.
      }
    }
    void load()
    const timer = window.setInterval(() => void load(), 1000)
    return () => {
      cancelled = true
      controller.abort()
      window.clearInterval(timer)
    }
  }, [session, token])
  return activity
}

function participantName(
  status: AgentSeatStatus | undefined,
  fallback: string
) {
  if (!status) return fallback
  return status.participant.kind === "human"
    ? status.participant.name
    : status.participant.harness === "claude_code"
      ? "Claude Code"
      : status.participant.harness === "codex"
        ? "Codex"
        : status.participant.harness === "cline"
          ? "Cline"
          : "Pi"
}

function participantHarness(status: AgentSeatStatus | undefined) {
  return status?.participant.kind === "agent"
    ? status.participant.harness
    : undefined
}

function participantSubtitle(
  status: AgentSeatStatus | undefined,
  rackCount: number
) {
  if (status?.participant.kind === "agent" && status.participant.model) {
    return `${status.participant.model} · ${rackCount} tiles`
  }
  return `${rackCount} tiles`
}

function statusLabel(status: AgentSeatStatus) {
  if (status.state === "waiting_for_human") return "Human turn"
  if (status.state === "thinking") return "Thinking"
  if (status.state === "failed") {
    return `Failed: ${(status.failureCode ?? "unknown error").replaceAll("_", " ")}`
  }
  return status.state.charAt(0).toUpperCase() + status.state.slice(1)
}

function GameWorkspace({
  connection,
  onAuthoritativeView,
  onCredentialReset,
  onReload,
  rules,
  session,
  snapshotError,
  token,
  view,
}: {
  connection: ConnectionState
  onAuthoritativeView: (view: GameView) => void
  onCredentialReset: () => void
  onReload: () => Promise<GameView | undefined>
  rules?: Ruleset
  session: GameSession
  snapshotError?: Error
  token: string
  view: GameView
}) {
  const navigate = useNavigate()
  const matchStatus = useAgentMatchStatus(session, token)
  const matchActivity = useAgentMatchActivity(session, token)
  const state = view.public.state
  const [draft, setDraft] = useState<MoveDraft>(EMPTY_MOVE_DRAFT)
  const [pending, setPending] = useState(false)
  const [actionError, setActionError] = useState<string>()
  const [blankCoordinate, setBlankCoordinate] = useState<Coordinate>()
  const values = useMemo(
    () => displayLetterValues(state.ruleset_id, rules),
    [rules, state.ruleset_id]
  )
  const premiums = useMemo(() => displayPremiums(rules), [rules])
  const canAct =
    connection === "live" &&
    snapshotError === undefined &&
    view.authority === "seat" &&
    view.seat === state.current_player &&
    state.phase === "active"
  const rack = view.rack ?? []
  const rackTiles: RackTile[] = rack.map((tile) => ({
    id: tile.id,
    letter: physicalLetter(tile),
    value: values.get(physicalLetter(tile)) ?? 0,
  }))
  const tileOwners = useMemo(
    () => tileOwnersFromEvents(view.public.events),
    [view.public.events]
  )

  const tiles: Record<string, BoardTile> = {}
  state.board.forEach((tile, index) => {
    if (tile) {
      tiles[`${Math.floor(index / 15)}-${index % 15}`] = {
        letter: tile.letter,
        owner: tileOwners.get(tile.tile_id),
        value: tile.is_blank ? 0 : values.get(tile.letter),
      }
    }
  })
  const stagedTiles: Record<string, BoardTile> = Object.fromEntries(
    draft.placements.map((placement) => [
      `${placement.coordinate.row}-${placement.coordinate.column}`,
      {
        letter: placement.tile.letter,
        value: placement.tile.is_blank ? 0 : values.get(placement.tile.letter),
        staged: true,
      },
    ])
  )
  const moves = toMoveRecords(view)
  const latestMove = moves[0]

  const chooseSquare = (row: number, column: number) => {
    const result = stageSelectedTile(draft, rack, { row, column })
    if (result.needsBlank) {
      setBlankCoordinate({ row, column })
    } else {
      setDraft(result.draft)
    }
  }

  const assignBlank = (letter: string) => {
    if (!blankCoordinate) return
    const result = stageSelectedTile(draft, rack, blankCoordinate, letter)
    setDraft(result.draft)
    setBlankCoordinate(undefined)
  }

  const submitAction = async (action: GameMove) => {
    setPending(true)
    setActionError(undefined)
    try {
      const next = await submitGameAction(session, token, {
        expected_version: state.version,
        turn_number: state.version,
        idempotency_key: `web-${crypto.randomUUID()}`,
        action,
      })
      onAuthoritativeView(next)
      setDraft(EMPTY_MOVE_DRAFT)
      requestAnimationFrame(() => {
        document.querySelector<HTMLElement>("[data-game-status]")?.focus()
      })
    } catch (caught) {
      if (classifySessionFailure(caught) === "conflict") {
        const refreshed = await onReload()
        if (refreshed) {
          setDraft(EMPTY_MOVE_DRAFT)
          setActionError(
            "The turn changed before submission. The latest authoritative board is now loaded."
          )
          requestAnimationFrame(() => {
            document.querySelector<HTMLElement>("[data-game-status]")?.focus()
          })
        }
      } else {
        setActionError(
          caught instanceof Error
            ? caught.message
            : "The referee rejected the action"
        )
      }
    } finally {
      setPending(false)
    }
  }

  return (
    <div className="min-h-svh bg-background">
      <WorkspaceHeader
        subtitle={`Game ${state.game_id} · ${view.authority} view`}
      />
      <main
        id="main-content"
        tabIndex={-1}
        className="mx-auto grid max-w-[1600px] items-start gap-3 p-3 sm:p-5 lg:grid-cols-[minmax(0,1fr)_18rem] xl:grid-cols-[15rem_minmax(0,1fr)_18rem]"
      >
        <p aria-atomic="true" aria-live="polite" className="sr-only">
          {connectionMessage(connection)}{" "}
          {latestMove ? moveSummary(latestMove) : ""}
        </p>
        <aside
          aria-label="Players and match configuration"
          className="grid gap-3 sm:grid-cols-2 lg:col-span-2 xl:col-span-1 xl:grid-cols-1"
        >
          {state.scores.map((score, index) => {
            const seat = SEATS[index]
            if (!seat) return null
            const participant = matchStatus?.seats[index]
            return (
              <PlayerCard
                active={
                  state.phase === "active" && state.current_player === seat
                }
                agent={participantName(participant, seatLabel(seat))}
                deadlineAt={
                  view.turnDeadline?.seat === seat
                    ? view.turnDeadline.deadlineAt
                    : undefined
                }
                harness={participantHarness(participant)}
                human={participant?.participant.kind === "human"}
                key={seat}
                observedAt={view.observedAt}
                score={score}
                seat={seat}
                status={participant ? statusLabel(participant) : undefined}
                subtitle={participantSubtitle(
                  participant,
                  state.rack_counts[index]
                )}
              />
            )
          })}
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
                <span className="font-medium">
                  {rulesetLabel(state.ruleset_id)}
                </span>
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
        <section
          aria-labelledby="live-board-title"
          className="min-w-0 lg:col-start-1 xl:col-start-2"
        >
          {connection !== "live" || snapshotError ? (
            <Alert
              className="mb-3"
              role={
                snapshotError &&
                classifySessionFailure(snapshotError) === "credential"
                  ? "alert"
                  : "status"
              }
              variant={
                snapshotError &&
                classifySessionFailure(snapshotError) === "credential"
                  ? "destructive"
                  : "default"
              }
            >
              <Unplug />
              <AlertTitle>
                {snapshotError &&
                classifySessionFailure(snapshotError) === "credential"
                  ? "Capability expired or revoked"
                  : "Showing the last authoritative board"}
              </AlertTitle>
              <AlertDescription>
                {snapshotError?.message ?? connectionMessage(connection)}
              </AlertDescription>
              <div className="col-start-2 mt-2 flex flex-wrap gap-2">
                {snapshotError &&
                classifySessionFailure(snapshotError) === "credential" ? (
                  <Button onClick={onCredentialReset} size="sm">
                    Use another credential
                  </Button>
                ) : (
                  <Button
                    onClick={() => void onReload()}
                    size="sm"
                    variant="outline"
                  >
                    <RefreshCw /> Retry now
                  </Button>
                )}
              </div>
            </Alert>
          ) : null}
          <Card className="gap-3" size="sm">
            <CardHeader className="border-b sm:grid-cols-[1fr_auto]">
              <div>
                <div className="mb-1 flex items-center gap-2">
                  <Badge>
                    {state.phase === "active"
                      ? `${seatLabel(state.current_player)} to move`
                      : "Finished"}
                  </Badge>
                  <span className="text-xs text-muted-foreground">
                    Version {state.version}
                  </span>
                </div>
                <CardTitle data-game-status id="live-board-title" tabIndex={-1}>
                  Live board
                </CardTitle>
                <CardDescription>
                  Server snapshots are the only authoritative state.
                </CardDescription>
              </div>
              <div className="flex items-center gap-2 self-center">
                <span>
                  <Badge
                    variant={connection === "live" ? "secondary" : "outline"}
                  >
                    {connection}
                  </Badge>
                </span>
                <GameClock
                  active={state.phase === "active"}
                  deadlineAt={view.turnDeadline?.deadlineAt}
                  label={seatLabel(state.current_player)}
                  observedAt={view.observedAt}
                />
              </div>
            </CardHeader>
            <CardContent>
              <GameBoard
                announcement={latestMove ? moveSummary(latestMove) : undefined}
                disabled={!canAct || pending || draft.mode !== "place"}
                onSquareSelect={canAct ? chooseSquare : undefined}
                premiums={premiums}
                stagedTiles={stagedTiles}
                tiles={tiles}
              />
            </CardContent>
          </Card>
          {view.authority === "seat" ? (
            <>
              <GameRack
                disabled={!canAct || pending}
                exchangeIds={draft.exchangeIds}
                label={`${view.seat ? seatLabel(view.seat) : "Seat"} rack`}
                mode={canAct ? draft.mode : "read_only"}
                onPlacedTileSelect={(tileId) =>
                  setDraft((current) => removePlacement(current, tileId))
                }
                onTileSelect={(tileId) =>
                  setDraft((current) => selectRackTile(current, tileId))
                }
                placedIds={draft.placements.map(
                  (placement) => placement.tile_id
                )}
                selectedTileId={draft.selectedTileId}
                tiles={rackTiles}
              />
              {actionError ? (
                <Alert className="mt-3" variant="destructive">
                  <AlertCircle />
                  <AlertTitle>Action not committed</AlertTitle>
                  <AlertDescription>{actionError}</AlertDescription>
                </Alert>
              ) : null}
              <GameControls
                disabled={!canAct}
                exchangeIds={draft.exchangeIds}
                mode={draft.mode}
                onAction={(action) => void submitAction(action)}
                onClear={() => setDraft(EMPTY_MOVE_DRAFT)}
                onModeChange={(mode) =>
                  setDraft((current) => setDraftMode(current, mode))
                }
                pending={pending}
                placementCount={draft.placements.length}
                placements={draft.placements}
              />
            </>
          ) : null}
          {view.authority === "spectator" && view.racks ? (
            <div className="grid gap-3 sm:grid-cols-2">
              {view.racks.map((spectatorRack, index) => (
                <GameRack
                  key={`${SEATS[index] ?? index}-rack`}
                  label={`${SEATS[index] ? seatLabel(SEATS[index]) : `Seat ${index + 1}`} rack`}
                  tiles={spectatorRack.map((tile) => ({
                    id: tile.id,
                    letter: physicalLetter(tile),
                    value: values.get(physicalLetter(tile)) ?? 0,
                  }))}
                />
              ))}
            </div>
          ) : null}
        </section>
        <aside
          aria-label="Move history and projection details"
          className="min-w-0 lg:col-start-2 lg:row-start-2 xl:col-start-3 xl:row-start-1"
        >
          <MoveHistory moves={moves} />
          {view.authority === "spectator" &&
          (matchStatus !== undefined || matchActivity !== undefined) ? (
            <div className="mt-3">
              <AgentConsole
                activeSeat={state.current_player}
                activity={matchActivity}
                seatNames={state.scores.map((_, index) => {
                  const seat = SEATS[index]
                  return participantName(
                    matchStatus?.seats[index],
                    seat ? seatLabel(seat) : `Seat ${index + 1}`
                  )
                })}
              />
            </div>
          ) : null}
          <Card className="mt-3" size="sm">
            <CardHeader className="border-b">
              <CardTitle>Projection boundary</CardTitle>
              <CardDescription>{view.authority} capability</CardDescription>
            </CardHeader>
            <CardContent className="text-xs leading-5 text-muted-foreground">
              {view.authority === "seat"
                ? `Only seat ${view.seat}'s rack is available.`
                : view.authority === "spectator"
                  ? "All current racks are available; the future bag and seed remain hidden."
                  : "Only public board and history data are available."}
            </CardContent>
          </Card>
          {view.authority === "spectator" && state.phase === "finished" ? (
            <Button
              className="mt-3 w-full"
              onClick={() =>
                navigate(`/games/${encodeURIComponent(state.game_id)}/replay`)
              }
              variant="outline"
            >
              <History /> Open recorded replay
            </Button>
          ) : null}
        </aside>
      </main>
      <BlankAssignmentDialog
        onAssign={assignBlank}
        onOpenChange={(open) => {
          if (!open) setBlankCoordinate(undefined)
        }}
        open={blankCoordinate !== undefined}
      />
    </div>
  )
}

function toMoveRecords(view: GameView): MoveRecord[] {
  return view.public.events
    .filter((event) => event.kind.type !== "created")
    .slice(-12)
    .reverse()
    .map((event) => {
      const eventSeat = SEATS.find((seat) => seat === event.kind.player)
      const player = eventSeat ? seatLabel(eventSeat) : "Unknown seat"
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
