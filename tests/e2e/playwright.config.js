// Browser end-to-end tests for the web player. Run with `cargo xtask e2e` (builds the page, makes fixtures,
// serves target/web and runs these), or `npx playwright test` here once `cargo xtask web` has run.
const { defineConfig, devices } = require("@playwright/test");
const path = require("node:path");

const PORT = process.env.RVP_E2E_PORT || "4173";
const root = path.resolve(__dirname, "../..");

module.exports = defineConfig({
  testDir: ".",
  testMatch: "*.spec.js",
  timeout: 90_000,
  expect: { timeout: 10_000 },
  fullyParallel: false,
  workers: 1,
  reporter: [["list"]],
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    // RVP_E2E_THREADS=0 runs the page on its single-threaded build even when the threaded one exists.
    storageState:
      process.env.RVP_E2E_THREADS === "0"
        ? { cookies: [{ name: "rvp_threads", value: "0", domain: "127.0.0.1", path: "/", expires: -1, httpOnly: false, secure: false, sameSite: "Lax" }], origins: [] }
        : undefined,
    viewport: { width: 1280, height: 720 },
    deviceScaleFactor: 1,
    trace: "retain-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
        viewport: { width: 1280, height: 720 },
        deviceScaleFactor: 1,
        // Playback starts without a click in tests, as it does after any user gesture in real use.
        launchOptions: { args: ["--autoplay-policy=no-user-gesture-required"] },
      },
    },
  ],
  webServer: {
    command: `cargo xtask serve --port ${PORT}`,
    cwd: root,
    url: `http://127.0.0.1:${PORT}/`,
    reuseExistingServer: true,
    timeout: 120_000,
  },
});
