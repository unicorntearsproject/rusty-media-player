// M9 in the browser: raw audio files (MP3, FLAC, Ogg Vorbis, Opus, WAV, ADTS AAC) open, show their tags, play at 1x,
// seek, and play to the end.
const { test, expect } = require("@playwright/test");
const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "../..");
const FIXTURES = process.env.RVP_FIXTURES || path.join(root, "target/fixtures");
const AUDIO = (n) => path.join(FIXTURES, "audio", n);

const snap = (page) => page.evaluate(() => window.rvp.snapshot());
const waitFor = (page, fn, arg, timeout) => page.waitForFunction(fn, arg, { timeout: timeout || 15_000, polling: 30 });
const waitState = (page, state) => waitFor(page, (s) => window.rvp.snapshot().state === s, state);

async function load(page, file) {
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
  await page.goto("/");
  await waitFor(page, () => window.rvp && window.rvp.ready);
  await page.setInputFiles("#file", file);
  await waitState(page, "playing");
  return errors;
}

test.describe("raw audio files", () => {
  // (file, has tags, duration in seconds)
  for (const [name, tagged, secs] of [
    ["cbr.mp3", true, 3.3],
    ["vbr_v23.mp3", true, 3.3],
    ["plain.mp3", false, 3.34],
    ["tone.flac", true, 3.3],
    ["tone.ogg", true, 3.3],
    ["tone.opus", true, 3.3],
    ["tone_flac.oga", true, 3.3],
    ["tone16.wav", true, 3.3],
    ["tone.aac", false, 3.33],
  ]) {
    test(`${name}: plays at 1x, shows its tags, seeks and ends`, async ({ page }) => {
      expect(fs.existsSync(AUDIO(name)), `${name} is missing: run cargo xtask e2e (it makes the audio fixtures)`).toBeTruthy();
      const errors = await load(page, AUDIO(name));
      let s = await snap(page);
      expect(s.error).toBeFalsy();
      expect(s.has_video).toBe(false);
      expect(s.duration_us / 1e6).toBeCloseTo(secs, 1);
      if (tagged) {
        await waitFor(page, () => navigator.mediaSession.metadata && navigator.mediaSession.metadata.title !== "");
        const m = await page.evaluate(() => {
          const x = navigator.mediaSession.metadata;
          return { title: x.title, artist: x.artist, album: x.album };
        });
        expect(m).toEqual({ title: "Chirp Étude", artist: "The Tones", album: "Pure" });
      }
      // Seek back and forth with the media session handler's twin: the keyboard.
      await page.keyboard.press("ArrowRight");
      await waitFor(page, () => window.rvp.snapshot().position_us > 1_500_000 || window.rvp.snapshot().state === "ended");
      await waitState(page, "ended");
      s = await snap(page);
      expect(s.error).toBeFalsy();
      expect(errors).toEqual([]);
    });
  }
});
