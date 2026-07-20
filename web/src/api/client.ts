import {
  decodeAgentCatalog,
  decodeAgentMatchStatus,
  decodeApiError,
  decodeCreatedAgentMatch,
  decodeCreatedGame,
  decodeGameView,
  decodeReplayBundle,
  decodeRuleset,
} from "@/api/decode"
import type {
  AgentCatalogEntry,
  AgentMatchStatus,
  CreateAgentMatchRequest,
  CreatedAgentMatch,
  CreatedGame,
  CreateGameRequest,
  GameActionRequest,
  GameSession,
  GameView,
  ReplayBundle,
  Ruleset,
} from "@/api/types"

export const DEFAULT_SERVER_ORIGIN =
  import.meta.env.VITE_WORD_ARENA_SERVER ?? "http://127.0.0.1:3000"

export class GameApiError extends Error {
  readonly code: string
  readonly status: number

  constructor(status: number, code: string, message: string) {
    super(message)
    this.name = "GameApiError"
    this.status = status
    this.code = code
  }
}

export async function createLocalGame(
  serverOrigin: string,
  request: CreateGameRequest,
  signal?: AbortSignal
): Promise<CreatedGame> {
  const response = await fetch(`${serverOrigin}/api/v1/games`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(request),
    cache: "no-store",
    signal,
  })
  return decodeCreatedGame(await responseBody(response))
}

export async function fetchAgentCatalog(
  serverOrigin: string,
  signal?: AbortSignal
): Promise<AgentCatalogEntry[]> {
  const response = await fetch(`${serverOrigin}/api/v1/agents`, {
    cache: "no-store",
    signal,
  })
  return decodeAgentCatalog(await responseBody(response))
}

export async function createAgentMatch(
  serverOrigin: string,
  request: CreateAgentMatchRequest,
  signal?: AbortSignal
): Promise<CreatedAgentMatch> {
  const response = await fetch(`${serverOrigin}/api/v1/matches`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(request),
    cache: "no-store",
    signal,
  })
  return decodeCreatedAgentMatch(await responseBody(response))
}

export async function fetchAgentMatchStatus(
  session: GameSession,
  token: string,
  signal?: AbortSignal
): Promise<AgentMatchStatus> {
  const response = await fetch(
    `${session.serverOrigin}/api/v1/matches/${encodeURIComponent(session.gameId)}`,
    {
      headers: { Authorization: `Bearer ${token}` },
      cache: "no-store",
      signal,
    }
  )
  return decodeAgentMatchStatus(await responseBody(response))
}

export async function fetchSpectatorReplay(
  session: GameSession,
  token: string,
  signal?: AbortSignal
): Promise<ReplayBundle> {
  if (session.authority !== "spectator") {
    throw new GameApiError(
      403,
      "spectator_required",
      "Replay requires a human-spectator capability"
    )
  }
  const response = await fetch(
    `${session.serverOrigin}/api/v1/games/${encodeURIComponent(session.gameId)}/spectator/replay`,
    {
      headers: { Authorization: `Bearer ${token}` },
      cache: "no-store",
      signal,
    }
  )
  return decodeReplayBundle(await responseBody(response))
}

export function normalizeServerOrigin(value: string): string {
  const url = new URL(value)
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    throw new Error("Server origin must use HTTP or HTTPS")
  }
  url.pathname = ""
  url.search = ""
  url.hash = ""
  return url.toString().replace(/\/$/, "")
}

export function snapshotPath(session: GameSession): string {
  const authorityPath =
    session.authority === "public" ? "public" : session.authority
  return `/api/v1/games/${encodeURIComponent(session.gameId)}/${authorityPath}`
}

export async function fetchGameView(
  session: GameSession,
  token: string,
  signal?: AbortSignal
): Promise<GameView> {
  const response = await fetch(
    `${session.serverOrigin}${snapshotPath(session)}`,
    {
      headers: { Authorization: `Bearer ${token}` },
      cache: "no-store",
      signal,
    }
  )
  const body: unknown = await response.json().catch(() => undefined)
  if (!response.ok) {
    const error = decodeApiError(body)
    throw new GameApiError(
      response.status,
      error?.code ?? "unexpected_response",
      error?.message ?? "The referee returned an unreadable error"
    )
  }
  return decodeGameView(body, session.authority)
}

async function responseBody(response: Response): Promise<unknown> {
  const body: unknown = await response.json().catch(() => undefined)
  if (!response.ok) {
    const error = decodeApiError(body)
    throw new GameApiError(
      response.status,
      error?.code ?? "unexpected_response",
      error?.message ?? "The referee returned an unreadable error"
    )
  }
  return body
}

export async function fetchRuleset(
  session: GameSession,
  token: string,
  signal?: AbortSignal
): Promise<Ruleset> {
  const response = await fetch(
    `${session.serverOrigin}/api/v1/games/${encodeURIComponent(session.gameId)}/rules`,
    {
      headers: { Authorization: `Bearer ${token}` },
      cache: "no-store",
      signal,
    }
  )
  return decodeRuleset(await responseBody(response))
}

export async function submitGameAction(
  session: GameSession,
  token: string,
  request: GameActionRequest,
  signal?: AbortSignal
): Promise<GameView> {
  if (session.authority !== "seat") {
    throw new GameApiError(403, "seat_required", "Only a private seat can act")
  }
  const response = await fetch(
    `${session.serverOrigin}/api/v1/games/${encodeURIComponent(session.gameId)}/actions`,
    {
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify(request),
      cache: "no-store",
      signal,
    }
  )
  const body = await responseBody(response)
  const envelope = body as { schema_version?: unknown; data?: unknown }
  const data = envelope.data as
    | { committed_at?: unknown; game?: unknown; turn_deadline?: unknown }
    | undefined
  return decodeGameView(
    {
      schema_version: envelope.schema_version,
      data: {
        observed_at: data?.committed_at,
        turn_deadline: data?.turn_deadline,
        game: data?.game,
      },
    },
    "seat"
  )
}
