import { queryOptions } from "@tanstack/react-query"

import { fetchGameView, fetchRuleset } from "@/api/client"
import { credentialVault } from "@/api/credentials"
import type { GameSession } from "@/api/types"

export function gameQueryKey(session: GameSession) {
  return [
    "game-snapshot",
    session.serverOrigin,
    session.gameId,
    session.authority,
  ] as const
}

export function gameQueryOptions(session: GameSession) {
  return queryOptions({
    queryKey: gameQueryKey(session),
    queryFn: ({ signal }) => {
      const token = credentialVault.get(session)
      if (!token) throw new Error("This game credential is no longer in memory")
      return fetchGameView(session, token, signal)
    },
    retry: (failureCount, error) => {
      const status =
        typeof error === "object" && error !== null && "status" in error
          ? error.status
          : undefined
      return status !== 401 && failureCount < 2
    },
    staleTime: 5_000,
  })
}

export function rulesQueryKey(session: GameSession) {
  return ["game-rules", session.serverOrigin, session.gameId] as const
}

export function rulesQueryOptions(session: GameSession) {
  return queryOptions({
    queryKey: rulesQueryKey(session),
    queryFn: ({ signal }) => {
      const token = credentialVault.get(session)
      if (!token) throw new Error("This game credential is no longer in memory")
      return fetchRuleset(session, token, signal)
    },
    staleTime: Number.POSITIVE_INFINITY,
    retry: false,
  })
}
