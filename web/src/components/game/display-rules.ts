import type { Premium, PublicGameState, Ruleset } from "@/api/types"

const englishValues = [
  1, 3, 3, 2, 1, 4, 2, 4, 1, 8, 5, 1, 3, 1, 1, 3, 10, 1, 1, 1, 1, 4, 4, 8, 4,
  10,
]
const frenchValues = [
  1, 3, 3, 2, 1, 4, 2, 4, 1, 8, 10, 1, 2, 1, 1, 3, 8, 1, 1, 1, 1, 4, 10, 10, 10,
  10,
]

const premiumCoordinates: Partial<Record<Premium, Array<[number, number]>>> = {
  double_letter: [
    [0, 3],
    [0, 11],
    [2, 6],
    [2, 8],
    [3, 0],
    [3, 7],
    [3, 14],
    [6, 2],
    [6, 6],
    [6, 8],
    [6, 12],
    [7, 3],
    [7, 11],
    [8, 2],
    [8, 6],
    [8, 8],
    [8, 12],
    [11, 0],
    [11, 7],
    [11, 14],
    [12, 6],
    [12, 8],
    [14, 3],
    [14, 11],
  ],
  triple_letter: [
    [1, 5],
    [1, 9],
    [5, 1],
    [5, 5],
    [5, 9],
    [5, 13],
    [9, 1],
    [9, 5],
    [9, 9],
    [9, 13],
    [13, 5],
    [13, 9],
  ],
  double_word: [
    [1, 1],
    [1, 13],
    [2, 2],
    [2, 12],
    [3, 3],
    [3, 11],
    [4, 4],
    [4, 10],
    [7, 7],
    [10, 4],
    [10, 10],
    [11, 3],
    [11, 11],
    [12, 2],
    [12, 12],
    [13, 1],
    [13, 13],
  ],
  triple_word: [
    [0, 0],
    [0, 7],
    [0, 14],
    [7, 0],
    [7, 14],
    [14, 0],
    [14, 7],
    [14, 14],
  ],
}

export function displayLetterValues(
  rulesetId: PublicGameState["ruleset_id"],
  rules?: Ruleset
): ReadonlyMap<string, number> {
  if (rules) {
    return new Map(
      rules.game.tiles.map((tile) => [
        tile.face.kind === "blank" ? "?" : tile.face.token,
        tile.value,
      ])
    )
  }
  const values = rulesetId === "french-v1" ? frenchValues : englishValues
  return new Map(
    values.map((value, index) => [String.fromCharCode(65 + index), value])
  ).set("?", 0)
}

export function displayPremiums(rules?: Ruleset): Record<string, Premium> {
  if (rules) {
    return Object.fromEntries(
      rules.game.board.squares.map((square) => [
        `${square.coordinate.row}-${square.coordinate.column}`,
        square.premium,
      ])
    )
  }
  const premiums: Record<string, Premium> = {}
  for (const [premium, coordinates] of Object.entries(
    premiumCoordinates
  ) as Array<[Premium, Array<[number, number]>]>) {
    for (const [row, column] of coordinates)
      premiums[`${row}-${column}`] = premium
  }
  return premiums
}
