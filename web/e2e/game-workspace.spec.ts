import AxeBuilder from "@axe-core/playwright"
import { expect, type Page, test } from "@playwright/test"

async function connect(
  page: Page,
  gameId: string,
  authority: "player" | "public" | "spectator",
  capability: string
) {
  await page.goto(`/games/${gameId}/${authority}`)
  await page.getByLabel("Capability").fill(capability)
  await page.getByRole("button", { name: "Open live game" }).click()
}

async function expectNoAxeViolations(page: Page) {
  const results = await new AxeBuilder({ page }).analyze()
  expect(
    results.violations.map(({ id, impact, nodes }) => ({
      id,
      impact,
      targets: nodes.flatMap((node) => node.target),
    }))
  ).toEqual([])
}

test("operator selects agents and starts directly into the spectator view", async ({
  page,
}) => {
  await page.goto("/")
  await expect(
    page.getByText("Build your lineup", { exact: true })
  ).toBeVisible()
  await expect(
    page.getByRole("combobox", { name: "Player 1 agent" })
  ).toBeVisible()
  await expect(
    page.getByRole("combobox", { name: "Player 2 agent" })
  ).toBeVisible()
  await expectNoAxeViolations(page)

  await page.getByRole("combobox", { name: "Player 2 agent" }).click()
  await page.getByRole("option", { name: /Claude Code/ }).click()
  await page.getByRole("button", { name: "Start match" }).click()
  await expect(page).toHaveURL(/\/games\/created-game\/spectator$/)
  await expect(page.getByText("Codex", { exact: true }).first()).toBeVisible()
  await expect(
    page.getByText("Claude Code", { exact: true }).first()
  ).toBeVisible()
  await expect(page.getByText("Seat one rack", { exact: true })).toBeVisible()
  await expect(page.getByText("Seat two rack", { exact: true })).toBeVisible()
  await expect(page.getByText("Agent activity", { exact: true })).toBeVisible()
  await expect(page.getByText("Turn 3 started", { exact: true })).toBeVisible()
  await expect(page.getByText(/15s live/)).toBeVisible()
  await expect(page.locator('[data-seat-owner="one"]')).toBeVisible()
  await page.getByRole("tab", { name: /Seat 2 Claude Code/ }).click()
  await expect(page.getByText("Placed E at H8.", { exact: true })).toBeVisible()
  const seatTwoEvents = page
    .getByRole("list", { name: "Seat two agent activity log" })
    .getByRole("listitem")
  await expect(seatTwoEvents.first()).toContainText(
    "Called word_arena.observe_game"
  )
  await expect(seatTwoEvents.last()).toContainText("Placed E at H8.")
  await expect(page.getByText("Turn 3 started", { exact: true })).toHaveCount(0)
  await page.getByRole("tab", { name: "Match" }).click()
  await expect(
    page.getByText("Agent-managed match created", { exact: true })
  ).toBeVisible()
  await expect(page.getByText("All current racks are available")).toBeVisible()
  await expectNoAxeViolations(page)
})

test("operator can expand a match from two to four players", async ({
  page,
}) => {
  await page.goto("/")
  await expect(page.getByText("🇬🇧 English", { exact: true })).toBeVisible()
  await expect(page.getByText("Player 1", { exact: true })).toBeVisible()
  await expect(page.getByText("Player 2", { exact: true })).toBeVisible()

  await page.getByRole("button", { name: "Add player 3" }).click()
  await page.getByRole("button", { name: "Add player 4" }).click()
  await expect(page.getByText("Player 3", { exact: true })).toBeVisible()
  await expect(page.getByText("Player 4", { exact: true })).toBeVisible()
  await expect(page.getByRole("button", { name: /Add player/ })).toHaveCount(0)

  await page.getByRole("button", { name: "Start match" }).click()
  await expect(page).toHaveURL(/\/games\/created-game\/spectator$/)
  for (const seat of ["one", "two", "three", "four"]) {
    await expect(
      page.getByText(`Seat ${seat} rack`, { exact: true })
    ).toBeVisible()
  }
  await expect(page.getByRole("tab", { name: /Seat 4 Codex/ })).toBeVisible()
  await expectNoAxeViolations(page)
})

test("local match tables reopen live and completed games after refresh", async ({
  page,
}) => {
  await page.goto("/")
  await expect(page.getByRole("tab", { name: /Live/ })).toBeVisible()
  await expect(page.getByText("spectator-live", { exact: true })).toBeVisible()
  await page.getByRole("tab", { name: /Finished/ }).click()
  await expect(page.getByText("replay-game", { exact: true })).toBeVisible()

  await page.getByRole("tab", { name: /Live/ }).click()

  await page.getByRole("button", { name: "Open spectator-live" }).click()
  await expect(page).toHaveURL(/\/games\/spectator-live\/spectator$/)
  await expect(page.getByText("Seat one rack", { exact: true })).toBeVisible()
  await page.reload()
  await expect(page.getByText("Seat one rack", { exact: true })).toBeVisible()
  await expect(page.getByLabel("Capability")).toHaveCount(0)

  await page.goto("/")
  await page.getByRole("tab", { name: /Finished/ }).click()
  await page.getByRole("button", { name: "Open replay-game" }).click()
  await expect(page).toHaveURL(/\/games\/replay-game\/replay$/)
  await expect(page.getByText("Exact replay inputs")).toBeVisible()
  await page.reload()
  await expect(page.getByText("Exact replay inputs")).toBeVisible()
  await expect(page.getByLabel("Capability")).toHaveCount(0)
  await expectNoAxeViolations(page)
})

test("operator can replace either agent seat with an optional human", async ({
  page,
}) => {
  await page.goto("/")
  await page.getByRole("switch", { name: "Use human for Player 1" }).click()
  await page.getByRole("textbox", { name: "Player name" }).fill("Reviewer")
  await expect(
    page.getByRole("switch", { name: "Use human for Player 2" })
  ).toBeDisabled()

  await page.getByRole("button", { name: "Start match" }).click()
  await expect(page).toHaveURL(/\/games\/created-game\/player$/)
  await expect(page.getByText("Reviewer", { exact: true })).toBeVisible()
  await expect(page.getByText("Seat one rack", { exact: true })).toBeVisible()
  await expect(page.getByText("Seat two rack", { exact: true })).toHaveCount(0)
  await expectNoAxeViolations(page)
})

test("player stages and commits a move without exposing another rack or a token", async ({
  page,
}) => {
  await connect(page, "player-active", "player", "seat-token")
  await expect(page.getByText("live", { exact: true })).toBeVisible()
  await expect(page.getByText("Seat one rack", { exact: true })).toBeVisible()
  await expect(page.getByText("Seat two rack", { exact: true })).toHaveCount(0)

  await page.getByRole("button", { name: "E, 1 points", exact: true }).click()
  await page.getByRole("button", { name: "I8: empty" }).click()
  await page.getByRole("button", { name: "Play move" }).click()
  await page.getByRole("button", { name: "Submit move" }).click()
  await expect(page.getByText("Version 4", { exact: true })).toBeVisible()
  await expect(
    page.getByText("Seat two to move", { exact: true })
  ).toBeVisible()

  expect(page.url()).not.toContain("seat-token")
  expect(
    await page.evaluate(() => ({
      local: Object.values(localStorage),
      session: Object.values(sessionStorage),
    }))
  ).toEqual({ local: [], session: [] })
  await expectNoAxeViolations(page)
})

test("spectator sees all current racks but no secret game inputs", async ({
  page,
}) => {
  await page.goto("/games/spectator-live/spectator")
  await expect(page.getByText("Seat one rack", { exact: true })).toBeVisible()
  await expect(page.getByText("Seat two rack", { exact: true })).toBeVisible()
  await expect(
    page.getByText(/future bag and seed remain hidden/)
  ).toBeVisible()
  await expect(page.getByRole("button", { name: "Play move" })).toHaveCount(0)
  await expectNoAxeViolations(page)
})

test("privacy fixture keeps an opponent rack outside a private seat projection", async ({
  page,
}) => {
  const response = page.waitForResponse((candidate) =>
    candidate.url().endsWith("/api/v1/games/privacy-game/seat")
  )
  await connect(page, "privacy-game", "player", "seat-token")
  const body = (await (await response).json()) as Record<string, unknown>
  const serialized = JSON.stringify(body)
  expect(serialized).not.toContain('"racks"')
  expect(serialized).not.toContain('"bag"')
  expect(serialized).not.toContain('"seed"')
  await expect(
    page.getByText("Only seat one's rack is available.")
  ).toBeVisible()
})

test("expired authority fails closed before any private state is rendered", async ({
  page,
}) => {
  await connect(page, "auth-failure", "player", "expired-token")
  await expect(
    page.getByText("Capability expired or revoked", { exact: true })
  ).toBeVisible()
  await expect(page.getByText("Seat one rack", { exact: true })).toHaveCount(0)
  await expect(
    page.getByRole("button", { name: "Use another credential" })
  ).toBeVisible()
  await expectNoAxeViolations(page)
})

test("finished spectator game opens its immutable replay with exact inputs", async ({
  page,
}) => {
  await page.goto("/games/terminal-game/spectator")
  await expect(
    page.getByLabel("Live board").getByText("Finished", { exact: true })
  ).toBeVisible()
  await page.getByRole("button", { name: "Open recorded replay" }).click()
  await expect(page).toHaveURL(/\/games\/terminal-game\/replay$/)
  await expect(page.getByText("Exact replay inputs")).toBeVisible()
  await expect(page.getByText(/word-arena-lexicon-en@1\.0\.0/)).toBeVisible()
  await expect(
    page.getByRole("button", { name: "Export public replay" })
  ).toBeVisible()
  await page.getByRole("button", { name: "First replay event" }).click()
  await expect(page.getByText("Event 1 of 4", { exact: true })).toBeVisible()
  await expectNoAxeViolations(page)
})

test("dropped invalidation stream reconnects and refreshes the authoritative version", async ({
  isMobile,
  page,
}) => {
  test.skip(
    isMobile,
    "The deterministic reconnect transition runs once per fixture server"
  )
  let socketCount = 0
  page.on("websocket", () => {
    socketCount += 1
  })
  await connect(page, "reconnect-game", "public", "public-token")
  await expect(page.getByText("live", { exact: true })).toBeVisible()
  await expect.poll(() => socketCount).toBeGreaterThan(1)
  await expect(page.getByText("Version 4", { exact: true })).toBeVisible({
    timeout: 5_000,
  })
  await expect(page.getByText("live", { exact: true })).toBeVisible()
})

test("mobile workspace keeps the board in its named horizontal scroll region", async ({
  isMobile,
  page,
}) => {
  test.skip(!isMobile, "Mobile-only viewport assertion")
  await page.goto("/games/spectator-live/spectator")
  const boardRegion = page.getByRole("region", {
    name: "Scrollable 15 by 15 word game board region",
  })
  await expect(boardRegion).toBeVisible()
  expect(
    await boardRegion.evaluate(
      (element) => element.scrollWidth > element.clientWidth
    )
  ).toBe(true)
})
