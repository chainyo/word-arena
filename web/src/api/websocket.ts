import { decodeInvalidation } from "@/api/decode"
import {
  type GameInvalidation,
  type GameSession,
  WEBSOCKET_PROTOCOL,
} from "@/api/types"

export const RECONNECT_DELAYS_MS = [250, 500, 1_000, 2_000, 5_000] as const

export function reconnectDelay(attempt: number): number {
  return RECONNECT_DELAYS_MS[
    Math.min(Math.max(attempt, 0), RECONNECT_DELAYS_MS.length - 1)
  ]
}

export function shouldInvalidate(
  marker: GameInvalidation,
  session: GameSession,
  snapshotVersion: number
): boolean {
  return marker.game_id === session.gameId && marker.version > snapshotVersion
}

export function websocketUrl(
  session: GameSession,
  afterVersion: number
): string {
  const url = new URL(session.serverOrigin)
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:"
  url.pathname = `/api/v1/games/${encodeURIComponent(session.gameId)}/events`
  url.searchParams.set("after_version", String(afterVersion))
  return url.toString()
}

type InvalidationSocketOptions = {
  getVersion: () => number
  onInvalidation: (marker: GameInvalidation) => void
  onStateChange: (
    state: "connecting" | "live" | "reconnecting" | "offline"
  ) => void
  session: GameSession
  token: string
}

export function connectInvalidationSocket({
  getVersion,
  onInvalidation,
  onStateChange,
  session,
  token,
}: InvalidationSocketOptions): () => void {
  let stopped = false
  let attempt = 0
  let socket: WebSocket | undefined
  let timer: number | undefined

  const connect = () => {
    if (stopped) return
    onStateChange(attempt === 0 ? "connecting" : "reconnecting")
    socket = new WebSocket(websocketUrl(session, getVersion()), [
      WEBSOCKET_PROTOCOL,
      token,
    ])
    socket.addEventListener("open", () => {
      attempt = 0
      onStateChange("live")
    })
    socket.addEventListener("message", (event) => {
      try {
        const marker = decodeInvalidation(JSON.parse(String(event.data)))
        if (shouldInvalidate(marker, session, getVersion())) {
          onInvalidation(marker)
        }
      } catch {
        socket?.close(1002, "invalid marker")
      }
    })
    socket.addEventListener("close", () => {
      if (stopped) return
      onStateChange(navigator.onLine ? "reconnecting" : "offline")
      timer = window.setTimeout(connect, reconnectDelay(attempt))
      attempt += 1
    })
  }

  connect()
  return () => {
    stopped = true
    if (timer !== undefined) window.clearTimeout(timer)
    socket?.close(1000, "route changed")
  }
}
