// HEVC (Main and Main 10) is decoded by our own decoder in every browser. 10-bit H.264 goes to the browser's decoders (WebCodecs): where
// this browser can decode it the picture plays; where it cannot (Playwright's Chromium) the sound plays on and the toast says what the
// codec is, why there is no picture and what to do. Both are asserted, whichever the machine is.
const { test, expect } = require("@playwright/test");
const path = require("node:path");
const fs = require("node:fs");
const { waitFor, waitState, frames } = require("./helpers");

const root = path.resolve(__dirname, "../..");
const FIXTURES = path.join(process.env.RVP_FIXTURES || path.join(root, "target/fixtures"), "hevc");
const have = (f) => fs.existsSync(path.join(FIXTURES, f));

async function open(page, file) {
  await page.goto("/");
  await waitFor(page, () => window.rvp && window.rvp.ready);
  await page.setInputFiles("#file", path.join(FIXTURES, file));
}

const cases = [
  ["hevc_aac.mp4", "hevc-main", /HEVC \(H\.265\)/, /Convert it to H\.264 or AV1/],
  ["hevc10_aac.mp4", "hevc-main10", /HEVC \(H\.265\)/, /Convert it to H\.264 or AV1/],
  ["hevc_flac.mkv", "hevc-main", /HEVC \(H\.265\)/, /Convert it to H\.264 or AV1/],
  ["h264_10bit.mp4", "h264-high10", /10-bit H\.264/, /Re-encode it as 8-bit/],
];

for (const [file, key, codecText, nextText] of cases) {
  test(`${file}: the picture plays (ours for HEVC, the browser's decoder for 10-bit H.264 where it has one), and the message is clear where it cannot`, async ({ page }) => {
    test.skip(!have(file), "no HEVC fixtures (needs ffmpeg with libx265)");
    await open(page, file);
    const pv = await page.evaluate(() => window.rvp.platformVideo());
    test.info().annotations.push({ type: "webcodecs", description: JSON.stringify(pv) });
    await waitState(page, "playing");
    if (key.startsWith("hevc") || (pv.supported && pv.supported[key])) {
      await waitFor(page, () => (window.rvp.snapshot().video || { presented: 0 }).presented > 5);
      const s = await page.evaluate(() => window.rvp.snapshot());
      expect(s.state).toBe("playing");
      // A seek back flushes the decoder; the picture goes on from the new place and the clock keeps following the sound.
      await waitFor(page, () => window.rvp.snapshot().position_us > 1_200_000);
      const before = (await page.evaluate(() => window.rvp.snapshot())).video.presented;
      await page.keyboard.press("ArrowLeft");
      await waitFor(page, (n) => window.rvp.snapshot().video.presented > n + 3, before);
      const t0 = (await page.evaluate(() => window.rvp.snapshot())).position_us;
      await waitFor(page, (t) => window.rvp.snapshot().position_us > t + 300000, t0);
    } else {
      // No picture, sound on, and a message that names the codec and the next step.
      await waitFor(page, () => /No picture/.test((window.rvp.snapshot().lib && window.rvp.snapshot().toast) || window.rvp.snapshot().toast || ""), null, 30_000);
      const toast = (await page.evaluate(() => window.rvp.snapshot())).toast;
      expect(toast).toMatch(codecText);
      expect(toast).toMatch(nextText);
      expect(toast).toMatch(/sound plays on/);
      await frames(page, 2);
      expect((await page.evaluate(() => window.rvp.snapshot())).state).toBe("playing");
    }
  });
}
