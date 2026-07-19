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

test("operator creates a game directly into the human spectator view", async ({
  page,
}) => {
  await page.goto("/")
  await expect(page.getByText("Create a game", { exact: true })).toBeVisible()
  await expectNoAxeViolations(page)

  await page.getByRole("button", { name: "Create and spectate" }).click()
  await expect(page).toHaveURL(/\/games\/created-game\/spectator$/)
  await expect(page.getByText("Seat one rack", { exact: true })).toBeVisible()
  await expect(page.getByText("Seat two rack", { exact: true })).toBeVisible()
  await expect(page.getByText("Both current racks are available")).toBeVisible()
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
  await expect(page.getByText("two to move", { exact: true })).toBeVisible()

  expect(page.url()).not.toContain("seat-token")
  expect(
    await page.evaluate(() => ({
      local: Object.values(localStorage),
      session: Object.values(sessionStorage),
    }))
  ).toEqual({ local: [], session: [] })
  await expectNoAxeViolations(page)
})

test("spectator sees both current racks but no secret game inputs", async ({
  page,
}) => {
  await connect(page, "spectator-live", "spectator", "spectator-token")
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
  await connect(page, "terminal-game", "spectator", "spectator-token")
  await expect(page.getByText("Finished", { exact: true })).toBeVisible()
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
  await connect(page, "reconnect-game", "public", "public-token")
  await expect(page.getByText("live", { exact: true })).toBeVisible()
  await expect(
    page.getByText("Showing the last authoritative board", { exact: true })
  ).toBeVisible({ timeout: 3_000 })
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
  await connect(page, "spectator-live", "spectator", "spectator-token")
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
