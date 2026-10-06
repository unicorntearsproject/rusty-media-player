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

const { snap, waitFor, waitState, frames, playedFor, settled } = require("./helpers");

/** Click the seek bar at `t` seconds and wait until the position is there (a paused player shows the picture and the subtitle of it). */
async function seekTo(page, t) {
  const s = await snap(page);
  await page.mouse.click(s.seek.x + s.seek.w * (t / (s.duration_us / 1e6)), s.seek.y + 2);
  await waitFor(page, (t) => Math.abs(window.rvp.snapshot().position_us / 1e6 - t) < 0.3, t);
  await frames(page, 3);
}

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
    // "Hello" is up from 1.0 s to 2.0 s: it shows no sooner than 1.0 s, and while the cue lasts (how soon after 1.0 s depends on how
    // busy the machine is: the headless tests check that to the frame).
    await waitFor(page, () => window.rvp.snapshot().subtitle === "Hello");
    const at = (await snap(page)).position_us;
    expect(at).toBeGreaterThan(900_000);
    expect(at).toBeLessThan(2_000_000);
    // The text is on the canvas: pause, go to the middle of the cue (a paused player shows the subtitle of its position, however long
    // the machine took to get here), then compare with the same frame without subtitles.
    await page.keyboard.press("Space");
    await waitState(page, "paused");
    await seekTo(page, 1.5);
    await waitFor(page, () => window.rvp.snapshot().subtitle === "Hello");
    const sz = s.surface;
    const region = [Math.round(sz.w * 0.25), Math.round(sz.h * 0.6), Math.round(sz.w * 0.5), Math.round(sz.h * 0.28)];
    const grab = () => page.evaluate((r) => window.rvp.sample(r[0], r[1], r[2], r[3], 2), region);
    await waitFor(page, () => window.rvp.snapshot().controls_opacity > 0.95);
    const withText = await grab();
    await page.keyboard.press("s"); // Spanish: a different text, still a cue? no: "Hola" is also 1-2 s
    await page.keyboard.press("s"); // off
    await waitFor(page, () => window.rvp.snapshot().subtitle === null);
    await frames(page, 4);
    const without = await grab();
    let changed = 0;
    for (let i = 0; i < withText.length; i += 3) {
      if (Math.abs(withText[i] - without[i]) + Math.abs(withText[i + 1] - without[i + 1]) > 60) changed++;
    }
    expect(changed / (withText.length / 3), "pixels drawn for the subtitle").toBeGreaterThan(0.008);
    // Back on; between the cues there is no text.
    await page.keyboard.press("s");
    await seekTo(page, 2.5);
    await waitFor(page, () => window.rvp.snapshot().subtitle === null);
    // Second track: Spanish ("Mundo" is up from 3.0 s to 4.5 s); then off.
    await page.keyboard.press("s");
    s = await snap(page);
    expect(s.selected_subtitle).toBe(s.subtitle_tracks[1].id);
    await seekTo(page, 3.7);
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
    await playedFor(page, 500_000);
    const p0 = (await snap(page)).position_us;
    await page.keyboard.press("a");
    await waitFor(page, (f) => window.rvp.snapshot().selected_audio !== f, first);
    await waitState(page, "playing");
    await playedFor(page, 500_000);
    s = await snap(page);
    expect(s.selected_audio).not.toBe(first);
    expect(s.position_us).toBeGreaterThan(p0);
    expect(errors).toEqual([]);
  });

  test("frame step and the A-B loop", async ({ page }) => {
    const errors = await load(page, H264);
    await page.keyboard.press("Space");
    await waitState(page, "paused");
    const position = () => window.rvp.snapshot().position_us;
    // The clock of a player that was paused stands between two pictures, wherever the machine let it get: the first step goes to a
    // picture, and the step that is measured goes from one picture to the next. (A step is an exact seek: let each one land.)
    const p00 = (await snap(page)).position_us;
    await page.keyboard.press(".");
    await waitFor(page, (p) => window.rvp.snapshot().position_us > p, p00);
    const p0 = await settled(page, position, { n: 4, gap: 3 });
    await page.keyboard.press(".");
    await waitFor(page, (p) => window.rvp.snapshot().position_us > p, p0);
    const p1 = await settled(page, position, { n: 4, gap: 3 });
    expect(p1 - p0).toBeGreaterThan(30_000);
    expect(p1 - p0).toBeLessThan(90_000);
    await page.keyboard.press(",");
    await waitFor(page, (p) => window.rvp.snapshot().position_us < p, p1);
    await settled(page, () => window.rvp.snapshot().position_us, { n: 4, gap: 3 });
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
    // Watch until the position has jumped back twice: it stays inside the loop (a little past B, as far as a slow frame lets it get).
    const out = await page.evaluate(async () => {
      let max = 0, jumps = 0, last = window.rvp.snapshot().position_us;
      const t0 = performance.now();
      while (jumps < 2 && performance.now() - t0 < 40_000) {
        const p = window.rvp.snapshot().position_us;
        if (p < last - 200_000) jumps++;
        max = Math.max(max, p);
        last = p;
        await new Promise((r) => setTimeout(r, 20));
      }
      return { max, jumps };
    });
    expect(out.max).toBeLessThan(s.loop_b + 1_000_000);
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
      while (performance.now() - t0 < 60_000) {
        const s = window.rvp.snapshot();
        if (titles[titles.length - 1] !== s.title) titles.push(s.title);
        if (s.state === "ended") break;
        await new Promise((r) => setTimeout(r, 20));
      }
      return titles;
    });
    expect(seen).toEqual(["gap_0.mkv", "gap_1.mkv", "gap_2.mkv"]);
    await waitState(page, "ended");
    // These are audio files, so the app showed its Library face; the playlist menu belongs to the Player (there Q shows the queue).
    await page.keyboard.press("b");
    await waitFor(page, () => window.rvp.snapshot().lib.mode === "player");
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

  test("the stable snapshot subset: version, ready, state, position, duration, item and error", async ({ page }) => {
    const errors = await load(page, LONG);
    const s = await snap(page);
    expect(s.version).toMatch(/^\d+\.\d+\.\d+/);
    expect(s.ready).toBe(true);
    expect(["opening", "playing", "paused", "buffering"]).toContain(s.state);
    expect(typeof s.position_us).toBe("number");
    expect(s.duration_us).toBeGreaterThan(0);
    expect(s.item).toMatchObject({ index: 0 });
    expect(typeof s.item.id).toBe("number");
    expect(s.item.title).toBeTruthy();
    expect(s.error).toBeNull();
    expect(await page.evaluate(() => window.rvp.ready)).toBe(true);
    expect(errors).toEqual([]);
  });

  test("resume: the position survives a reload", async ({ page }) => {
    const errors = await load(page, LONG);
    const s = await snap(page);
    await page.mouse.click(s.seek.x + s.seek.w * 0.5, s.seek.y + 2);
    await waitFor(page, () => Math.abs(window.rvp.snapshot().position_us / 1e6 - 30) < 2);
    await frames(page, 3);
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

  test("Media Session: tags, art and state reach the browser, and its buttons control the player", async ({ page }) => {
    // Capture the handlers the page registers so the test can press the "media keys".
    await page.addInitScript(() => {
      window.__msHandlers = {};
      const ms = navigator.mediaSession;
      const orig = ms.setActionHandler.bind(ms);
      ms.setActionHandler = (a, f) => { window.__msHandlers[a] = f; return orig(a, f); };
    });
    const errors = await load(page, M8("tagged.m4a"));
    const meta = () => page.evaluate(() => {
      const m = navigator.mediaSession.metadata;
      return m && { title: m.title, artist: m.artist, album: m.album, art: m.artwork.length, state: navigator.mediaSession.playbackState };
    });
    await waitFor(page, () => navigator.mediaSession.metadata && navigator.mediaSession.metadata.title === "Sine Song");
    let m = await meta();
    expect(m).toEqual({ title: "Sine Song", artist: "The Tones", album: "Pure", art: 1, state: "playing" });
    // The cover is a real image the browser can decode.
    const ok = await page.evaluate(async () => {
      const img = new Image();
      img.src = navigator.mediaSession.metadata.artwork[0].src;
      await img.decode();
      return [img.width, img.height];
    });
    expect(ok).toEqual([64, 64]);
    // Pause, seek and play through the handlers.
    await page.evaluate(() => window.__msHandlers.pause());
    await waitState(page, "paused");
    await waitFor(page, () => navigator.mediaSession.playbackState === "paused");
    await page.evaluate(() => window.__msHandlers.seekto({ seekTime: 3 }));
    await waitFor(page, () => Math.abs(window.rvp.snapshot().position_us / 1e6 - 3) < 0.4);
    await page.evaluate(() => window.__msHandlers.play());
    await waitState(page, "playing");
    await waitFor(page, () => navigator.mediaSession.playbackState === "playing");
    expect(errors).toEqual([]);
  });

  test("Media Session: next and previous track move through the playlist", async ({ page }) => {
    await page.addInitScript(() => {
      window.__msHandlers = {};
      const ms = navigator.mediaSession;
      const orig = ms.setActionHandler.bind(ms);
      ms.setActionHandler = (a, f) => { window.__msHandlers[a] = f; return orig(a, f); };
    });
    const errors = await load(page, [M8("gap_0.mkv"), M8("gap_1.mkv")]);
    await waitFor(page, () => navigator.mediaSession.metadata && navigator.mediaSession.metadata.title === "gap_0");
    await page.evaluate(() => window.__msHandlers.nexttrack());
    await waitFor(page, () => navigator.mediaSession.metadata.title === "gap_1");
    expect((await snap(page)).title).toBe("gap_1.mkv");
    await page.evaluate(() => window.__msHandlers.previoustrack());
    await waitFor(page, () => navigator.mediaSession.metadata.title === "gap_0");
    expect(errors).toEqual([]);
  });

  test("visualizer tap: the page sees the analysis of what it plays (a sine's band, clicks as onsets and a tempo)", async ({ page }) => {
    const errors = await boot(page);
    await page.evaluate(() => window.rvp.visualizer(true));
    await page.setInputFiles("#file", M8("clicks_120.mkv"));
    await waitState(page, "playing");
    await waitFor(page, () => window.rvp.vizState() && window.rvp.vizState().onsets >= 8, undefined, 20_000);
    const v = await page.evaluate(() => window.rvp.vizState());
    expect(v.summaries).toBeGreaterThan(300);
    expect(v.last.bands).toHaveLength(32);
    await waitFor(page, () => window.rvp.vizState().last.tempo_bpm > 0, undefined, 20_000);
    const t = await page.evaluate(() => window.rvp.vizState().last.tempo_bpm);
    expect(Math.abs(t - 120)).toBeLessThan(3);
    // Off again: nothing is analysed.
    await page.evaluate(() => window.rvp.visualizer(false));
    expect(await page.evaluate(() => window.rvp.vizState())).toBeNull();
    expect(errors).toEqual([]);
  });

  test("chapters: Page Down and Page Up move between the marks, the menu lists them", async ({ page }) => {
    for (const file of ["chapters.mkv", "chapters.mp4"]) {
      const errors = await load(page, M8(file));
      let s = await snap(page);
      expect(s.chapters.map((c) => c.title)).toEqual(["Intro", "Middle", "End"]);
      await page.keyboard.press("Space");
      await waitState(page, "paused");
      await page.keyboard.press("PageDown");
      await waitFor(page, () => Math.abs(window.rvp.snapshot().position_us / 1e6 - 2) < 0.3);
      await page.keyboard.press("PageDown");
      await waitFor(page, () => Math.abs(window.rvp.snapshot().position_us / 1e6 - 4) < 0.3);
      await page.keyboard.press("PageUp"); // near the start of "End": back to "Middle"
      await waitFor(page, () => Math.abs(window.rvp.snapshot().position_us / 1e6 - 2) < 0.3);
      // The menu: right click, Chapters submenu lists them; clicking one seeks there.
      await page.mouse.click(300, 200, { button: "right" });
      await waitFor(page, () => window.rvp.snapshot().menu_open);
      s = await snap(page);
      const parent = s.menu.find((m) => m.label === "Chapters");
      expect(parent.enabled).toBe(true);
      await page.mouse.move(parent.rect.x + 20, parent.rect.y + parent.rect.h / 2);
      await waitFor(page, () => window.rvp.snapshot().menu.some((m) => m.label.endsWith("Intro")));
      s = await snap(page);
      const row = s.menu.find((m) => m.label.endsWith("Intro"));
      await page.mouse.click(row.rect.x + 10, row.rect.y + row.rect.h / 2);
      await waitFor(page, () => window.rvp.snapshot().position_us < 300_000);
      expect(errors).toEqual([]);
    }
  });
});
