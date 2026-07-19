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
  let socketGeneration = 0
  let timer: number | undefined

  const clearTimer = () => {
    if (timer !== undefined) {
      window.clearTimeout(timer)
      timer = undefined
    }
  }

  const scheduleReconnect = () => {
    if (stopped) return
    clearTimer()
    if (!navigator.onLine) {
      onStateChange("offline")
      return
    }
    onStateChange("reconnecting")
    const delay = reconnectDelay(attempt)
    attempt += 1
    timer = window.setTimeout(connect, delay)
  }

  function connect() {
    if (stopped) return
    clearTimer()
    if (!navigator.onLine) {
      onStateChange("offline")
      return
    }
    onStateChange(attempt === 0 ? "connecting" : "reconnecting")
    const generation = ++socketGeneration
    const nextSocket = new WebSocket(websocketUrl(session, getVersion()), [
      WEBSOCKET_PROTOCOL,
      token,
    ])
    socket = nextSocket
    nextSocket.addEventListener("open", () => {
      if (generation !== socketGeneration) return
      attempt = 0
      onStateChange("live")
    })
    nextSocket.addEventListener("message", (event) => {
      if (generation !== socketGeneration) return
      try {
        const marker = decodeInvalidation(JSON.parse(String(event.data)))
        if (shouldInvalidate(marker, session, getVersion())) {
          onInvalidation(marker)
        }
      } catch {
        nextSocket.close(1002, "invalid marker")
      }
    })
    nextSocket.addEventListener("close", () => {
      if (generation !== socketGeneration) return
      scheduleReconnect()
    })
  }

  const handleOnline = () => {
    if (stopped) return
    socketGeneration += 1
    socket?.close(1000, "network restored")
    socket = undefined
    attempt = 0
    connect()
  }
  const handleOffline = () => {
    clearTimer()
    onStateChange("offline")
    socketGeneration += 1
    socket?.close(1001, "browser offline")
    socket = undefined
  }

  window.addEventListener("online", handleOnline)
  window.addEventListener("offline", handleOffline)
  connect()
  return () => {
    stopped = true
    socketGeneration += 1
    clearTimer()
    window.removeEventListener("online", handleOnline)
    window.removeEventListener("offline", handleOffline)
    socket?.close(1000, "route changed")
  }
}
