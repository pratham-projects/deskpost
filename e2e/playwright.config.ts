import { defineConfig } from "@playwright/test"

export default defineConfig({
  testDir: "./tests",
  workers: 1,
  fullyParallel: false,
  timeout: 90_000,
  reporter: [["list"]],
  use: {
    baseURL: process.env.E2E_BASE_URL,
    channel: process.env.E2E_CHROME_CHANNEL || "chrome",
    trace: "retain-on-failure",
  },
})
