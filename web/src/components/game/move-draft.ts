import type { Coordinate, PhysicalTile, Placement } from "@/api/types"

export type DraftPlacement = Placement

export type MoveDraft = {
  selectedTileId?: number
  placements: DraftPlacement[]
  exchangeIds: number[]
  mode: "place" | "exchange"
}

export const EMPTY_MOVE_DRAFT: MoveDraft = {
  placements: [],
  exchangeIds: [],
  mode: "place",
}

export function physicalLetter(tile: PhysicalTile): string {
  return tile.face.kind === "blank" ? "?" : tile.face.token
}

export function selectRackTile(draft: MoveDraft, tileId: number): MoveDraft {
  if (draft.mode === "exchange") return toggleExchangeTile(draft, tileId)
  return {
    ...draft,
    selectedTileId: draft.selectedTileId === tileId ? undefined : tileId,
  }
}

export function toggleExchangeTile(
  draft: MoveDraft,
  tileId: number
): MoveDraft {
  const selected = draft.exchangeIds.includes(tileId)
    ? draft.exchangeIds.filter((id) => id !== tileId)
    : [...draft.exchangeIds, tileId]
  return { ...draft, exchangeIds: selected, selectedTileId: undefined }
}

export function setDraftMode(
  draft: MoveDraft,
  mode: MoveDraft["mode"]
): MoveDraft {
  return {
    ...draft,
    mode,
    selectedTileId: undefined,
    exchangeIds: mode === "exchange" ? draft.exchangeIds : [],
    placements: mode === "place" ? draft.placements : [],
  }
}

export function stageSelectedTile(
  draft: MoveDraft,
  rack: PhysicalTile[],
  coordinate: Coordinate,
  blankLetter?: string
): { draft: MoveDraft; needsBlank: boolean } {
  const tile = rack.find((candidate) => candidate.id === draft.selectedTileId)
  if (!tile) return { draft, needsBlank: false }
  if (tile.face.kind === "blank" && blankLetter === undefined) {
    return { draft, needsBlank: true }
  }
  const letter = tile.face.kind === "blank" ? blankLetter : tile.face.token
  if (!letter || !/^[A-Z]$/.test(letter)) {
    return { draft, needsBlank: tile.face.kind === "blank" }
  }
  const placements = draft.placements.filter(
    (placement) =>
      placement.tile_id !== tile.id &&
      (placement.coordinate.row !== coordinate.row ||
        placement.coordinate.column !== coordinate.column)
  )
  placements.push({
    tile_id: tile.id,
    coordinate,
    tile: { letter, is_blank: tile.face.kind === "blank" },
  })
  placements.sort(
    (left, right) =>
      left.coordinate.row - right.coordinate.row ||
      left.coordinate.column - right.coordinate.column
  )
  return {
    needsBlank: false,
    draft: { ...draft, placements, selectedTileId: undefined },
  }
}

export function removePlacement(draft: MoveDraft, tileId: number): MoveDraft {
  return {
    ...draft,
    placements: draft.placements.filter(
      (placement) => placement.tile_id !== tileId
    ),
  }
}
