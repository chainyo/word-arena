import { defineConfig, devices } from "@playwright/test"

const webOrigin = "http://127.0.0.1:4173"

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  workers: process.env.CI ? 2 : 1,
  reporter: process.env.CI ? "github" : "list",
  outputDir: "test-results",
  use: {
    baseURL: webOrigin,
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
    video: "off",
  },
  projects: [
    {
      name: "desktop-chromium",
      use: { ...devices["Desktop Chrome"] },
    },
    {
      name: "mobile-chromium",
      use: { ...devices["Pixel 7"] },
    },
  ],
  webServer: [
    {
      command: "bun run fixtures",
      port: 4174,
      reuseExistingServer: !process.env.CI,
      timeout: 30_000,
    },
    {
      command:
        "VITE_WORD_ARENA_SERVER=http://127.0.0.1:4174 bun run dev -- --host 127.0.0.1 --port 4173",
      port: 4173,
      reuseExistingServer: !process.env.CI,
      timeout: 30_000,
    },
  ],
})
