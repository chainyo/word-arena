import type { GameEvent, Seat } from "@/api/types"

function eventSeat(value: unknown): Seat | undefined {
  return value === "one" ||
    value === "two" ||
    value === "three" ||
    value === "four"
    ? value
    : undefined
}

/** Rebuilds committed tile ownership from the authoritative public event log. */
export function tileOwnersFromEvents(
  events: GameEvent[],
  throughSequence = Number.POSITIVE_INFINITY
): Map<number, Seat> {
  const owners = new Map<number, Seat>()

  for (const event of events) {
    if (event.sequence > throughSequence || event.kind.type !== "move_played") {
      continue
    }
    const seat = eventSeat(event.kind.player)
    if (!seat || !Array.isArray(event.kind.placements)) continue

    for (const value of event.kind.placements) {
      if (typeof value !== "object" || value === null) continue
      const tileId = (value as { tile_id?: unknown }).tile_id
      if (typeof tileId === "number") owners.set(tileId, seat)
    }
  }

  return owners
}
