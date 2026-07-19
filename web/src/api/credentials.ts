import type { GameSession } from "@/api/types"

function sessionKey(session: GameSession): string {
  return `${session.serverOrigin}\u0000${session.gameId}\u0000${session.authority}`
}

class MemoryCredentialVault {
  readonly #credentials = new Map<string, string>()

  get(session: GameSession): string | undefined {
    return this.#credentials.get(sessionKey(session))
  }

  set(session: GameSession, token: string): void {
    const trimmed = token.trim()
    if (!trimmed || trimmed.length > 256) {
      throw new Error("Capability must contain between 1 and 256 characters")
    }
    this.#credentials.set(sessionKey(session), trimmed)
  }

  delete(session: GameSession): void {
    this.#credentials.delete(sessionKey(session))
  }

  clear(): void {
    this.#credentials.clear()
  }
}

/**
 * Credentials intentionally live only in process memory. They are never
 * copied into URLs, localStorage, sessionStorage, IndexedDB, or query keys.
 */
export const credentialVault = new MemoryCredentialVault()
