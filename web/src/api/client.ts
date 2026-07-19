import { decodeApiError, decodeGameView } from "@/api/decode"
import type { GameSession, GameView } from "@/api/types"

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
