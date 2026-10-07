// Audio must not stutter while the page is busy: a song, then a film, plays while the poster grid scrolls and menus open and close. The
// AudioWorklet counts the silences it had to play (`window.rvp.audio().underFrames`, runs in `underRuns`) and the fewest frames its ring held
// (`ringLow`); this checks the counters, not the ears. The main thread is the page's single place of drawing, so a slow frame (software
// rendering, a big grid) must be absorbed by the audio's lead in the ring, not heard.
const { test, expect } = require("@playwright/test");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const root = path.resolve(__dirname, "../..");
const FIXTURES = process.env.RVP_FIXTURES || path.join(root, "target/fixtures");
const { snap, waitFor: waitForAt, frames, press, toFace } = require("./helpers");
const waitFor = (page, fn, arg, timeout) => waitForAt(page, fn, arg, timeout, 30);

const SONG = path.join(FIXTURES, "levels/music.mp3"); // 30 s
const FILM = path.join(FIXTURES, "av1_opus_60s.webm"); // 60 s
const FILM_1080 = path.join(FIXTURES, "perf/h264_1080p30_typ.mp4"); // 1080p30 H.264 + AAC, the film that taxes the page most

/** A folder `Films` with `n` small films (copies of one fixture), so the poster grid is long enough to scroll. */
function manyFilms(n) {
  const dir = path.join(os.tmpdir(), `rvp-underrun-e2e-${process.pid}`, "Films");
  fs.mkdirSync(dir, { recursive: true });
  for (let i = 0; i < n; i++) {
    fs.copyFileSync(path.join(FIXTURES, "h264_aac.mp4"), path.join(dir, `Film ${String(i + 1).padStart(2, "0")}.mp4`));
  }
  return dir;
}

/** A slow machine, on request: `RVP_UNDERRUN_THROTTLE=6` makes the page's main thread six times slower (Chrome's CPU throttling). */
async function boot(page, throttle = Number(process.env.RVP_UNDERRUN_THROTTLE || 4)) {
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto(process.env.RVP_AUDIOHINT ? `/?audiohint=${process.env.RVP_AUDIOHINT}` : "/");
  await waitFor(page, () => window.rvp && window.rvp.ready);
  if (throttle > 1) {
    const cdp = await page.context().newCDPSession(page);
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: throttle });
  }
  return errors;
}

const audio = (page) => page.evaluate(() => window.rvp.audio());

/** Everything the page can be asked to redraw heavily, for a number of rounds: scroll the grid, open and close the menu and the settings. */
async function stress(page, rounds) {
  const box = await page.evaluate(() => ({ w: window.innerWidth, h: window.innerHeight }));
  for (let i = 0; i < rounds; i++) {
    await page.mouse.move(box.w * 0.5, box.h * 0.5);
    await page.mouse.wheel(0, i % 2 ? -320 : 320);
    await frames(page, 1);
    await page.mouse.wheel(0, i % 2 ? -160 : 160);
    await frames(page, 1);
    // The app menu (the hamburger's key) and back; the settings sheet and back.
    await page.keyboard.press("F10");
    await frames(page, 1);
    await page.keyboard.press("Escape");
    await frames(page, 1);
    await page.keyboard.press("Control+,");
    await frames(page, 1);
    await page.keyboard.press("Escape");
    await frames(page, 1);
    // A burst of pointer moves over the grid: hover states redraw cards.
    for (let k = 0; k < 8; k++) await page.mouse.move(box.w * (0.2 + 0.08 * k), box.h * (0.3 + 0.05 * ((i + k) % 6)));
  }
}

// Software rendering, as on the machines where this went wrong: no GPU at all (the page's drawing then competes with everything else for the CPU).
test.use({ launchOptions: { args: ["--autoplay-policy=no-user-gesture-required", "--disable-gpu"] } });

test.describe("audio stays whole while the page redraws", () => {
  test.describe.configure({ mode: "serial" });

  test("a song plays through scrolling the poster grid and opening menus without a silence", async ({ page }) => {
    const errors = await boot(page);
    await toFace(page, "library");
    await page.setInputFiles("#dir", manyFilms(24));
    await waitFor(page, () => {
      const l = window.rvp.snapshot().lib;
      return l.videos === 24 && l.posters === 24 && !l.scan;
    }, null, 90_000);
    await press(page, "8");
    await waitFor(page, () => window.rvp.snapshot().lib.view === "videos");
    await page.setInputFiles("#file", SONG);
    await waitFor(page, () => window.rvp.snapshot().state === "playing" && window.rvp.snapshot().position_us > 1_500_000, null, 30_000);
    // Back to the grid while the song plays.
    await press(page, "8");
    await waitFor(page, () => window.rvp.snapshot().lib.view === "videos");
    await page.evaluate(() => window.rvp.audioReset());
    const a0 = await audio(page);
    await stress(page, Number(process.env.RVP_UNDERRUN_ROUNDS || 12));
    const a1 = await audio(page);
    const s = await snap(page);
    console.log(`song: hint ${a1.hint}, ring low ${a1.ringLow} frames, underruns +${a1.underRuns - a0.underRuns} (${a1.underFrames - a0.underFrames} frames), state ${s.state}`);
    expect(s.state).toBe("playing");
    expect(a1.underFrames - a0.underFrames).toBe(0);
    // The ring never ran low: at least a quarter of a second stayed queued through the slowest frame (and the page was four times slower).
    expect(a1.ringLow).toBeGreaterThan(12_000);
    expect(errors).toEqual([]);
  });

  test("a film plays through the same without a silence", async ({ page }) => {
    const errors = await boot(page);
    await page.setInputFiles("#file", FILM);
    await waitFor(page, () => window.rvp.snapshot().state === "playing" && window.rvp.snapshot().position_us > 1_500_000, null, 30_000);
    await page.evaluate(() => window.rvp.audioReset());
    const a0 = await audio(page);
    // The controls, the menu, a seek bar drag and the settings: everything that redraws the player face over a playing film.
    const box = await page.evaluate(() => ({ w: window.innerWidth, h: window.innerHeight }));
    for (let i = 0; i < Number(process.env.RVP_UNDERRUN_ROUNDS || 12); i++) {
      await page.mouse.move(box.w * (0.3 + 0.04 * (i % 8)), box.h * 0.9);
      await frames(page, 1);
      await page.keyboard.press("F10");
      await frames(page, 1);
      await page.keyboard.press("Escape");
      await frames(page, 1);
      await page.keyboard.press("Control+,");
      await frames(page, 1);
      await page.keyboard.press("Escape");
      await frames(page, 1);
    }
    const a1 = await audio(page);
    console.log(`film: hint ${a1.hint}, ring low ${a1.ringLow} frames, underruns +${a1.underRuns - a0.underRuns} (${a1.underFrames - a0.underFrames} frames)`);
    expect(a1.underFrames - a0.underFrames).toBe(0);
    expect(errors).toEqual([]);
  });

  test("the controls act at once however much audio is queued", async ({ page }) => {
    const errors = await boot(page, 1);
    await page.setInputFiles("#file", SONG);
    await waitFor(page, () => window.rvp.snapshot().state === "playing" && window.rvp.audio().ring > 20_000, null, 30_000);
    // Volume (the output's gain node), pause (the ring is held) and seek (the ring is flushed): each is in effect a couple of frames later,
    // not after what is queued has played out.
    const v0 = (await audio(page)).volume;
    await press(page, "ArrowDown");
    const v1 = (await audio(page)).volume;
    expect(v1).toBeLessThan(v0);
    await press(page, "m");
    expect((await audio(page)).volume).toBe(0);
    await press(page, "m");
    expect((await audio(page)).volume).toBe(v1);
    await press(page, " ");
    const paused = await audio(page);
    expect(paused.paused).toBe(true);
    // (One status report may already have been on its way; after a few frames the count is final.)
    await frames(page, 6);
    const p0 = (await audio(page)).played;
    await frames(page, 12);
    expect((await audio(page)).played).toBe(p0);
    await press(page, " ");
    await waitFor(page, () => window.rvp.snapshot().state === "playing");
    // A seek flushes: the worklet restarts its counter and the ring refills from the new place.
    const before = (await audio(page)).played;
    await press(page, "ArrowRight", 4);
    await waitFor(page, (b) => window.rvp.audio().played < b, before, 5_000);
    expect(errors).toEqual([]);
  });

  test("a 1080p film keeps its audio whole while menus open and the grid redraws (software rendering, a slow machine)", async ({ page }) => {
    test.skip(!fs.existsSync(FILM_1080), "the perf fixtures are not generated");
    const errors = await boot(page);
    await page.setInputFiles("#file", FILM_1080);
    await waitFor(page, () => window.rvp.snapshot().state === "playing" && window.rvp.snapshot().position_us > 1_500_000, null, 60_000);
    // Let the start-up fill settle, then count from here.
    await page.waitForTimeout(1500);
    await page.evaluate(() => window.rvp.audioReset());
    const a0 = await audio(page);
    const box = await page.evaluate(() => ({ w: window.innerWidth, h: window.innerHeight }));
    const rounds = Number(process.env.RVP_UNDERRUN_ROUNDS || 14);
    // The counters are read after every round, as long as the film is not within two seconds of its end (the end of a film is silence).
    let last = a0;
    let rode = 0;
    for (let i = 0; i < rounds; i++) {
      await page.mouse.move(box.w * (0.3 + 0.04 * (i % 8)), box.h * 0.9);
      await frames(page, 1);
      await page.keyboard.press("F10");
      await frames(page, 1);
      await page.keyboard.press("Escape");
      await frames(page, 1);
      await page.keyboard.press("Control+,");
      await frames(page, 1);
      await page.keyboard.press("Escape");
      await frames(page, 1);
      const s = await page.evaluate(() => window.rvp.snapshot());
      if (s.state !== "playing" || s.duration_us - s.position_us < 2_000_000) break;
      last = await audio(page);
      rode = s.position_us;
    }
    console.log(`1080p film: through ${(rode / 1e6).toFixed(1)} s, ring low ${last.ringLow} frames, underruns +${last.underRuns - a0.underRuns} (${last.underFrames - a0.underFrames} frames)`);
    expect(rode).toBeGreaterThan(8_000_000); // it played a good while under the stress
    expect(last.underFrames - a0.underFrames).toBe(0);
    expect(last.ringLow).toBeGreaterThan(0);
    // The ring kept at least 200 ms of the 900 ms the player aims for, even on the single-thread build with a 1080p film at a quarter speed.
    expect(last.ringLow).toBeGreaterThan(9_600);
    expect(errors).toEqual([]);
  });
});
