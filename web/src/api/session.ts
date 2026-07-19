import type { ConnectionState } from "@/api/types"

export type SessionFailure = "credential" | "conflict" | "network" | "server"

function errorField(error: unknown, field: "code" | "status") {
  return typeof error === "object" && error !== null && field in error
    ? (error as Record<string, unknown>)[field]
    : undefined
}

export function classifySessionFailure(error: unknown): SessionFailure {
  const status = errorField(error, "status")
  const code = errorField(error, "code")
  if (status === 401 || code === "unauthorized") return "credential"
  if (code === "version_conflict" || (status === 409 && code === undefined)) {
    return "conflict"
  }
  if (error instanceof TypeError) {
    return "network"
  }
  return "server"
}

export function connectionMessage(connection: ConnectionState): string {
  switch (connection) {
    case "live":
      return "Connected to the referee."
    case "connecting":
      return "Connecting to the referee."
    case "reconnecting":
      return "Connection interrupted. Reconnecting with bounded backoff."
    case "offline":
      return "Offline. The last board is stale and actions are disabled."
  }
}
