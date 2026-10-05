// M8 in the browser: subtitles (embedded and sidecar), audio tracks, frame stepping, the A-B loop, the playlist
// (gapless chain, next/previous) and resume after a reload.
const { test, expect } = require("@playwright/test");
const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "../..");
const FIXTURES = process.env.RVP_FIXTURES || path.join(root, "target/fixtures");
const M8 = (n) => path.join(FIXTURES, "m8", n);
const LONG = path.join(FIXTURES, "av1_opus_60s.webm");
const H264 = path.join(FIXTURES, "h264_aac.mp4"); // 6 s, 25 fps

const snap = (page) => page.evaluate(() => window.rvp.snapshot());
const waitFor = (page, fn, arg, timeout) => page.waitForFunction(fn, arg, { timeout: timeout || 15_000, polling: 30 });
const waitState = (page, state) => waitFor(page, (s) => window.rvp.snapshot().state === s, state);

async function boot(page) {
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
  await page.goto("/");
  await waitFor(page, () => window.rvp && window.rvp.ready);
  return errors;
}

async function load(page, files) {
  const errors = await boot(page);
  await page.setInputFiles("#file", files);
  await waitState(page, "playing");
  return errors;
}

test.describe("M8", () => {
  test("embedded subtitles: S cycles tracks, the text shows at the right time and is drawn", async ({ page }) => {
    const errors = await load(page, M8("subs_srt.mkv"));
    let s = await snap(page);
    expect(s.subtitle_tracks.map((t) => t.label)).toEqual(["English", "Spanish"]);
    expect(s.selected_subtitle).toBeNull();
    expect(s.subtitle).toBeNull();
    await page.keyboard.press("s");
    s = await snap(page);
    expect(s.selected_subtitle).toBe(s.subtitle_tracks[0].id);
    // "Hello" is up from 1.0 s to 2.0 s.
    await waitFor(page, () => window.rvp.snapshot().subtitle === "Hello");
    const at = (await snap(page)).position_us;
    expect(at).toBeGreaterThan(900_000);
    expect(at).toBeLessThan(1_300_000);
    // The text is on the canvas: pause inside the cue, then compare with the same frame without subtitles.
    await page.keyboard.press("Space");
    await waitState(page, "paused");
    expect((await snap(page)).subtitle).toBe("Hello");
    const sz = s.surface;
    const region = [Math.round(sz.w * 0.25), Math.round(sz.h * 0.6), Math.round(sz.w * 0.5), Math.round(sz.h * 0.28)];
    const grab = () => page.evaluate((r) => window.rvp.sample(r[0], r[1], r[2], r[3], 2), region);
    await waitFor(page, () => window.rvp.snapshot().controls_opacity > 0.95);
    const withText = await grab();
    await page.keyboard.press("s"); // Spanish: a different text, still a cue? no: "Hola" is also 1-2 s
    await page.keyboard.press("s"); // off
    await waitFor(page, () => window.rvp.snapshot().subtitle === null);
    await page.waitForTimeout(150);
    const without = await grab();
    let changed = 0;
    for (let i = 0; i < withText.length; i += 3) {
      if (Math.abs(withText[i] - without[i]) + Math.abs(withText[i + 1] - without[i + 1]) > 60) changed++;
    }
    expect(changed / (withText.length / 3), "pixels drawn for the subtitle").toBeGreaterThan(0.008);
    // Back on, play on until the cue is gone.
    await page.keyboard.press("s");
    await page.keyboard.press("Space");
    await waitFor(page, () => window.rvp.snapshot().subtitle === null && window.rvp.snapshot().position_us > 2_100_000);
    // Second track: Spanish; then off.
    await page.keyboard.press("s");
    s = await snap(page);
    expect(s.selected_subtitle).toBe(s.subtitle_tracks[1].id);
    await waitFor(page, () => window.rvp.snapshot().subtitle === "Mundo");
    await page.keyboard.press("s");
    expect((await snap(page)).selected_subtitle).toBeNull();
    expect(errors).toEqual([]);
  });

  test("a dropped .srt joins the video that is playing", async ({ page }) => {
    const errors = await load(page, [M8("subs_movtext.mp4")]);
    await page.setInputFiles("#file", [M8("subs_movtext.mp4"), M8("sub_es.srt")]);
    await waitState(page, "playing");
    await waitFor(page, () => window.rvp.snapshot().subtitle_tracks.some((t) => t.label === "sub_es.srt"));
    const s = await snap(page);
    expect(s.selected_subtitle).toBe(s.subtitle_tracks.find((t) => t.label === "sub_es.srt").id);
    await waitFor(page, () => window.rvp.snapshot().subtitle === "Hola");
    expect(errors).toEqual([]);
  });

  test("audio tracks: A switches to the next track and playback goes on", async ({ page }) => {
    const errors = await load(page, M8("two_audio.mkv"));
    let s = await snap(page);
    expect(s.audio_tracks.map((t) => t.label.split(" ")[0])).toEqual(["English", "Spanish"]);
    const first = s.selected_audio;
    await page.waitForTimeout(600);
    const p0 = (await snap(page)).position_us;
    await page.keyboard.press("a");
    await waitFor(page, (f) => window.rvp.snapshot().selected_audio !== f, first);
    await waitState(page, "playing");
    await page.waitForTimeout(800);
    s = await snap(page);
    expect(s.selected_audio).not.toBe(first);
    expect(s.position_us).toBeGreaterThan(p0);
    expect(errors).toEqual([]);
  });

  test("frame step and the A-B loop", async ({ page }) => {
    const errors = await load(page, H264);
    await page.keyboard.press("Space");
    await waitState(page, "paused");
    const p0 = (await snap(page)).position_us;
    await page.keyboard.press(".");
    await waitFor(page, (p) => window.rvp.snapshot().position_us > p, p0);
    const p1 = (await snap(page)).position_us;
    expect(p1 - p0).toBeGreaterThan(30_000);
    expect(p1 - p0).toBeLessThan(90_000);
    await page.keyboard.press(",");
    await waitFor(page, (p) => window.rvp.snapshot().position_us < p, p1);
    expect((await snap(page)).state).toBe("paused");
    // The loop: mark 1 s and 2 s, play, and the position never gets far past B.
    await page.keyboard.press("Home");
    await waitFor(page, () => window.rvp.snapshot().position_us < 100_000);
    await page.keyboard.press("Space");
    await waitFor(page, () => window.rvp.snapshot().position_us > 1_000_000);
    await page.keyboard.press("i");
    await waitFor(page, () => window.rvp.snapshot().position_us > 2_000_000);
    await page.keyboard.press("i");
    let s = await snap(page);
    expect(s.loop_a).toBeGreaterThan(900_000);
    expect(s.loop_b).toBeGreaterThan(s.loop_a);
    // Watch for 3.5 s: the position stays inside the loop and jumps back at least twice.
    const out = await page.evaluate(async () => {
      let max = 0, jumps = 0, last = window.rvp.snapshot().position_us;
      const t0 = performance.now();
      while (performance.now() - t0 < 3500) {
        const p = window.rvp.snapshot().position_us;
        if (p < last - 200_000) jumps++;
        max = Math.max(max, p);
        last = p;
        await new Promise((r) => setTimeout(r, 20));
      }
      return { max, jumps };
    });
    expect(out.max).toBeLessThan(s.loop_b + 400_000);
    expect(out.jumps).toBeGreaterThanOrEqual(2);
    await page.keyboard.press("i");
    s = await snap(page);
    expect(s.loop_a).toBeNull();
    expect(errors).toEqual([]);
  });

  test("playlist: several files play back to back, N and P move between them, the menu lists them", async ({ page }) => {
    const errors = await load(page, [M8("gap_0.mkv"), M8("gap_1.mkv"), M8("gap_2.mkv")]);
    let s = await snap(page);
    expect(s.playlist.map((p) => p.label)).toEqual(["gap_0.mkv", "gap_1.mkv", "gap_2.mkv"]);
    expect(s.title).toBe("gap_0.mkv");
    await page.keyboard.press("n");
    await waitFor(page, () => window.rvp.snapshot().title === "gap_1.mkv");
    await page.keyboard.press("p");
    await waitFor(page, () => window.rvp.snapshot().title === "gap_0.mkv");
    // Let it run: the next items start by themselves, in order, and playback ends after the last.
    const seen = await page.evaluate(async () => {
      const titles = [];
      const t0 = performance.now();
      while (performance.now() - t0 < 12_000) {
        const s = window.rvp.snapshot();
        if (titles[titles.length - 1] !== s.title) titles.push(s.title);
        if (s.state === "ended") break;
        await new Promise((r) => setTimeout(r, 20));
      }
      return titles;
    });
    expect(seen).toEqual(["gap_0.mkv", "gap_1.mkv", "gap_2.mkv"]);
    await waitState(page, "ended");
    // The playlist menu lists the items and marks the current one.
    await page.keyboard.press("q");
    await waitFor(page, () => window.rvp.snapshot().menu_open);
    s = await snap(page);
    expect(s.menu.map((m) => m.label)).toEqual(expect.arrayContaining(["gap_0.mkv", "gap_1.mkv", "gap_2.mkv", "Add files…"]));
    const row = s.menu.find((m) => m.label === "gap_1.mkv");
    await page.mouse.click(row.rect.x + row.rect.w / 2, row.rect.y + row.rect.h / 2);
    await waitFor(page, () => window.rvp.snapshot().title === "gap_1.mkv");
    expect(errors).toEqual([]);
  });

  test("resume: the position survives a reload", async ({ page }) => {
    const errors = await load(page, LONG);
    const s = await snap(page);
    await page.mouse.click(s.seek.x + s.seek.w * 0.5, s.seek.y + 2);
    await waitFor(page, () => Math.abs(window.rvp.snapshot().position_us / 1e6 - 30) < 2);
    await page.waitForTimeout(500);
    await page.evaluate(() => window.rvp.saveState());
    await page.reload();
    await waitFor(page, () => window.rvp && window.rvp.ready);
    await page.setInputFiles("#file", LONG);
    await waitState(page, "playing");
    await waitFor(page, () => window.rvp.snapshot().position_us > 25_000_000, undefined, 20_000);
    const p = (await snap(page)).position_us / 1e6;
    expect(p).toBeGreaterThan(28);
    expect(p).toBeLessThan(36);
    expect(errors).toEqual([]);
  });
});
