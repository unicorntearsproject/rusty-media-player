// M9 real-time check: play each one-minute 1080p30 stream (`cargo xtask perf-fixtures`) at 1x in the browser and count
// dropped frames. Skipped unless RVP_PERF=1 (it takes a minute per stream); `cargo xtask perf-web` runs it.
//   RVP_PERF=1              run
//   RVP_PERF_SECS=60        seconds to play each stream
//   RVP_PERF_ONLY=h264      only streams whose file name contains this
//   RVP_PERF_QUERY=threads=0  extra query string for the page (the threaded build is the default when available)
//   RVP_PERF_LIMIT=2        fail above this share of dropped frames, in percent (0 to only report)
const { test, expect } = require("@playwright/test");
const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "../..");
const FIXTURES = process.env.RVP_FIXTURES || path.join(root, "target/fixtures");
const DIR = path.join(FIXTURES, "perf");
const SECS = Number(process.env.RVP_PERF_SECS || 60);
const LIMIT = Number(process.env.RVP_PERF_LIMIT ?? 2);
const ONLY = process.env.RVP_PERF_ONLY || "";
const QUERY = process.env.RVP_PERF_QUERY || "";

const files = fs.existsSync(DIR)
  ? fs.readdirSync(DIR).filter((f) => /\.(mp4|webm|mkv)$/.test(f) && f.includes(ONLY)).sort()
  : [];

test.describe("real-time 1080p30", () => {
  test.skip(!process.env.RVP_PERF, "set RVP_PERF=1 (and run `cargo xtask perf-fixtures` once)");
  test.setTimeout((SECS + 60) * 1000);

  for (const name of files) {
    test(`${name} plays ${SECS} s with few dropped frames`, async ({ page }) => {
      const errors = [];
      page.on("pageerror", (e) => errors.push(String(e)));
      page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
      await page.goto("/" + (QUERY ? "?" + QUERY : ""));
      await page.waitForFunction(() => window.rvp && window.rvp.ready, null, { timeout: 30_000 });
      const threads = await page.evaluate(() => (window.rvp.threads ? window.rvp.threads() : 0));
      await page.setInputFiles("#file", path.join(DIR, name));
      await page.waitForFunction(() => window.rvp.snapshot().state === "playing", null, { timeout: 30_000 });
      // Count from the first presented picture, so start-up work does not count.
      await page.waitForFunction(() => (window.rvp.snapshot().video || { presented: 0 }).presented > 2, null, { timeout: 30_000 });
      const base = await page.evaluate(() => window.rvp.snapshot());
      await page.waitForTimeout(SECS * 1000);
      const end = await page.evaluate(() => window.rvp.snapshot());
      const presented = end.video.presented - base.video.presented;
      const dropped = end.video.dropped - base.video.dropped;
      const pct = (100 * dropped) / Math.max(1, presented + dropped);
      const played = end.position_us / 1e6 - base.position_us / 1e6;
      const p = end.perf;
      console.log(
        `PERF ${name} threads=${threads} presented=${presented} dropped=${dropped} (${pct.toFixed(2)}%) ` +
          `played=${played.toFixed(1)}s max_tick=${p.max_tick_ms}ms session=${(p.session_ms / p.ticks).toFixed(2)}ms/tick ` +
          `render=${(p.render_ms / p.ticks).toFixed(2)} base=${(p.base_ms / p.ticks).toFixed(2)} present=${(p.present_ms / p.ticks).toFixed(2)} ` +
          `warnings=${JSON.stringify(end.warnings)}`
      );
      expect(errors).toEqual([]);
      expect(end.state).toBe("playing");
      if (LIMIT > 0) expect(pct).toBeLessThan(LIMIT);
    });
  }
});
