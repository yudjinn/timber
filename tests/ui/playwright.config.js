// End-to-end tests of the web build (dist/, from `trunk build`). See README.md.
import { defineConfig } from "@playwright/test";

const port = Number(process.env.TIMBER_UI_PORT || 8095);

export default defineConfig({
  testDir: "specs",
  timeout: 120_000,
  expect: { timeout: 15_000 },
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : [["list"]],
  globalSetup: "./fixtures.js",
  use: {
    baseURL: `http://127.0.0.1:${port}/`,
    viewport: { width: 1400, height: 900 },
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
  },
  webServer: {
    command: `node serve.js ${port}`,
    url: `http://127.0.0.1:${port}/`,
    reuseExistingServer: !process.env.CI,
  },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
});
