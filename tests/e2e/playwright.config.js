// Browser end-to-end tests for the web player. Run with `cargo xtask e2e` (builds the page, makes fixtures,
// serves target/web and runs these), or `npx playwright test` here once `cargo xtask web` has run.
const { defineConfig, devices } = require("@playwright/test");
const path = require("node:path");

const PORT = process.env.RVP_E2E_PORT || "4173";
const root = path.resolve(__dirname, "../..");

module.exports = defineConfig({
  testDir: ".",
  testMatch: "*.spec.js",
  // Each run of the suite can have a folder of its own for traces and videos (`cargo xtask perf-web` gives each pass one), so passes that
  // follow each other never clean up under one another.
  outputDir: process.env.RVP_E2E_OUTPUT || "test-results",
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
    // Headless always, so a run never opens a window on the user's display; RVP_HEADED=1 opts in to a visible browser.
    headless: process.env.RVP_HEADED !== "1",
    viewport: { width: 1280, height: 720 },
    deviceScaleFactor: 1,
    trace: "retain-on-failure",
  },
  projects: [
    {
      name: "chromium",
      // The audio stress test has a project of its own, below.
      testIgnore: /audio-underrun\.spec\.js/,
      use: {
        ...devices["Desktop Chrome"],
        viewport: { width: 1280, height: 720 },
        deviceScaleFactor: 1,
        // Playback starts without a click in tests, as it does after any user gesture in real use.
        launchOptions: { args: ["--autoplay-policy=no-user-gesture-required"] },
      },
    },
    {
      // The audio stress test throttles the page to a quarter of a CPU on purpose, so it measures the app only when nothing else of the
      // suite runs beside it: it starts after every other test has finished.
      name: "audio-stress",
      testMatch: /audio-underrun\.spec\.js/,
      dependencies: ["chromium"],
      use: {
        ...devices["Desktop Chrome"],
        viewport: { width: 1280, height: 720 },
        deviceScaleFactor: 1,
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
