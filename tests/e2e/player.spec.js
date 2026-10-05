// M5 "done when": open the AV1 fixture from a File and check the picture moves, the position advances at 1x,
// Space pauses, a seek click lands, every shortcut has a menu entry (also unit-tested in rvp-ui), and the
// paused screen matches a committed golden. Plus keyboard, context menu, speed, drag and drop, auto-hide and
// reduced-motion checks.
const { test, expect } = require("@playwright/test");
const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "../..");
const FIXTURES = process.env.RVP_FIXTURES || path.join(root, "target/fixtures");
const LONG = path.join(FIXTURES, "av1_opus_60s.webm"); // 60 s, 320x240, 25 fps, AV1 + Opus
const SHORT = path.join(FIXTURES, "av1_opus.webm"); // 6 s
const GOLDEN = path.join(__dirname, "golden", "player-paused.png");
const H264_AAC = path.join(FIXTURES, "h264_aac.mp4"); // 6 s, 320x240, x264 Main/High with B-frames + AAC
const H264_FLAC = path.join(FIXTURES, "h264_flac.mkv"); // the same video in Matroska with FLAC
const VP9_OPUS = path.join(FIXTURES, "vp9", "av_opus.webm"); // 6 s, 320x240, libvpx VP9 + Opus
const VP9_VORBIS = path.join(FIXTURES, "vp9_vorbis.webm"); // 6 s, 320x240, VP9 + Vorbis
const VP9_RESIZE = path.join(FIXTURES, "vp9", "r_keyframe.webm"); // 1.5 s, 320x240 then 480x270 then 200x120, video only

const { snap, waitFor, waitState, frames, ticks, settled, playedFor, rateAgainstDevice } = require("./helpers");

async function load(page, file = LONG, { play = true } = {}) {
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
  await page.goto("/");
  await waitFor(page, () => window.rvp && window.rvp.ready);
  expect((await snap(page)).state).toBe("idle");
  // The hidden <input type=file> is what the Open button clicks: this hands the page a real File.
  await page.setInputFiles("#file", file);
  await waitState(page, "playing");
  // Wait for the first picture.
  await waitFor(page, () => (window.rvp.snapshot().video || { presented: 0 }).presented > 3);
  if (!play) await page.keyboard.press("Space");
  return errors;
}

/** RGB samples of the inner part of the picture (clear of the controls). */
async function picture(page) {
  const s = await snap(page);
  const v = s.video_rect;
  const r = [Math.round(v.x + v.w * 0.1), Math.round(v.y + v.h * 0.12), Math.round(v.w * 0.8), Math.round(v.h * 0.5)];
  return page.evaluate((r) => window.rvp.sample(r[0], r[1], r[2], r[3], 4), r);
}

const diff = (a, b, thr = 12) => {
  let n = 0;
  for (let i = 0; i < a.length; i += 3) {
    if (Math.max(Math.abs(a[i] - b[i]), Math.abs(a[i + 1] - b[i + 1]), Math.abs(a[i + 2] - b[i + 2])) > thr) n++;
  }
  return n / (a.length / 3);
};

/** Wait until the picture differs from `before` by more than `min` of the sampled pixels; resolves with the new samples. */
async function pictureChange(page, before, min = 0.01, timeout = 30_000) {
  const t0 = Date.now();
  for (;;) {
    const now = await picture(page);
    if (diff(before, now) > min) return now;
    if (Date.now() - t0 > timeout) return now;
    await frames(page, 3);
  }
}

const spread = (a) => {
  let lo = 255, hi = 0;
  for (let i = 0; i < a.length; i += 3) { lo = Math.min(lo, a[i + 1]); hi = Math.max(hi, a[i + 1]); }
  return hi - lo;
};

test.describe("player", () => {
  test.beforeAll(() => {
    for (const f of [LONG, SHORT]) {
      expect(fs.existsSync(f), `${f} is missing: run cargo xtask fixtures`).toBeTruthy();
    }
  });

  test("a) the picture moves, b) position runs at 1x, c) Space pauses and keeps the last frame, d) a seek click lands", async ({ page }) => {
    const errors = await load(page);
    expect((await snap(page)).has_video).toBe(true);

    // (a) canvas pixels at the video rect change over 2 s.
    const before = await picture(page);
    expect(spread(before)).toBeGreaterThan(40); // it is a picture, not a flat colour
    const after = await pictureChange(page, before);
    const changed = diff(before, after);
    expect(changed, `only ${(changed * 100).toFixed(1)}% of the sampled pixels changed in 2 s`).toBeGreaterThan(0.01);

    // The audio clock is what drives playback: the worklet is running and its played-frame counter moves.
    const a1 = await page.evaluate(() => window.rvp.audio());
    expect(a1.mode).toBe("worklet");
    expect(a1.state).toBe("running");
    await waitFor(page, (n) => window.rvp.audio().played > n, a1.played + 10_000);

    // (b) The position advances at 1x +- 5% of the audio device's own clock.
    const run = await rateAgainstDevice(page, 3_000_000);
    expect(run.ratio, `position moved ${(run.moved / 1000).toFixed(0)} ms in ${(run.device / 1000).toFixed(0)} ms of device time`).toBeGreaterThan(0.95);
    expect(run.ratio).toBeLessThan(1.05);

    // (c) Space pauses: position frozen, the last frame stays on screen.
    await page.keyboard.press("Space");
    await waitState(page, "paused");
    await ticks(page, 20);
    const p1 = (await snap(page)).position_us;
    const frozen1 = await picture(page);
    await ticks(page, 50);
    const p2 = (await snap(page)).position_us;
    const frozen2 = await picture(page);
    expect(p2).toBe(p1);
    expect(diff(frozen1, frozen2)).toBe(0);
    expect(spread(frozen2)).toBeGreaterThan(40);

    // (d) A click on the seek bar lands within 1 s of the clicked point.
    const s = await snap(page);
    const target = 0.5 * (s.duration_us / 1e6);
    await page.mouse.click(s.seek.x + s.seek.w * 0.5, s.seek.y + s.seek.h / 2);
    await waitFor(page, (t) => Math.abs(window.rvp.snapshot().position_us / 1e6 - t) < 1, target);
    const landed = (await snap(page)).position_us / 1e6;
    expect(Math.abs(landed - target)).toBeLessThan(1);
    // ... and the preview picture follows the seek.
    const seekedDiff = diff(frozen2, await pictureChange(page, frozen2));
    expect(seekedDiff, "the picture shows the new position").toBeGreaterThan(0.01);
    // Playing resumes from there.
    await page.keyboard.press("Space");
    await waitState(page, "playing");
    const resumed = (await playedFor(page, 1_000_000)) / 1e6;
    expect(resumed).toBeGreaterThan(landed + 0.8);
    expect(resumed).toBeLessThan(landed + 6);
    expect(errors).toEqual([]);
  });

  for (const [codec, name, file] of [
    ["H.264", "MP4 + AAC", H264_AAC],
    ["H.264", "Matroska + FLAC", H264_FLAC],
    ["VP9", "WebM + Opus", VP9_OPUS],
    ["VP9", "WebM + Vorbis", VP9_VORBIS],
  ]) {
    test(`${codec} (${name}): shows a moving picture at 1x and a seek lands`, async ({ page }) => {
      expect(fs.existsSync(file), `${file} is missing: run cargo xtask fixtures`).toBeTruthy();
      const errors = await load(page, file);
      const s0 = await snap(page);
      expect(s0.has_video).toBe(true);
      expect(s0.error).toBeFalsy();
      const before = await picture(page);
      expect(spread(before)).toBeGreaterThan(40);
      const after = await pictureChange(page, before);
      expect(diff(before, after), "the picture changes while playing").toBeGreaterThan(0.01);
      const run = await rateAgainstDevice(page, 2_000_000);
      expect(run.ratio, `position moved ${(run.moved / 1000).toFixed(0)} ms in ${(run.device / 1000).toFixed(0)} ms of device time`).toBeGreaterThan(0.95);
      expect(run.ratio).toBeLessThan(1.05);
      // Pause, seek to the middle and make sure the frame there decodes (reference chains restart cleanly).
      await page.keyboard.press("Space");
      await waitState(page, "paused");
      await waitFor(page, () => window.rvp.snapshot().controls_opacity > 0.95);
      const held = await picture(page);
      const s = await snap(page);
      await page.mouse.click(s.seek.x + s.seek.w * 0.7, s.seek.y + 2);
      await waitFor(page, (t) => Math.abs(window.rvp.snapshot().position_us / 1e6 - t) < 1, 0.7 * (s.duration_us / 1e6));
      const moved = diff(held, await pictureChange(page, held));
      expect(moved, "the picture shows the new position").toBeGreaterThan(0.01);
      expect(errors).toEqual([]);
    });
  }

  test("VP9: a resolution change at a key frame plays through and the picture follows it", async ({ page }) => {
    expect(fs.existsSync(VP9_RESIZE), `${VP9_RESIZE} is missing: run cargo xtask fixtures`).toBeTruthy();
    const errors = [];
    page.on("pageerror", (e) => errors.push(String(e)));
    page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
    await page.goto("/");
    await waitFor(page, () => window.rvp && window.rvp.ready);
    // Record every picture size the page shows from the moment the file opens.
    await page.evaluate(() => {
      window.__sizes = [];
      setInterval(() => {
        const f = window.rvp.snapshot().frame_size;
        const last = window.__sizes[window.__sizes.length - 1];
        if (f && (!last || last[0] !== f[0] || last[1] !== f[1])) window.__sizes.push(f);
      }, 10);
    });
    await page.setInputFiles("#file", VP9_RESIZE);
    await waitState(page, "ended");
    const sizes = await page.evaluate(() => window.__sizes);
    expect(sizes).toEqual([[320, 240], [480, 270], [200, 120]]);
    const s = await snap(page);
    expect(s.error).toBeFalsy();
    expect(s.video.presented).toBeGreaterThan(30);
    expect(errors).toEqual([]);
  });

  test("keyboard: arrows, j/k/l, m, f, Home/End", async ({ page }) => {
    await load(page, LONG, { play: false });
    const pos = async () => (await snap(page)).position_us / 1e6;
    const at = async (t) => {
      const s = await snap(page);
      await page.mouse.click(s.seek.x + s.seek.w * (t / (s.duration_us / 1e6)), s.seek.y + 2);
      await waitFor(page, (t) => Math.abs(window.rvp.snapshot().position_us / 1e6 - t) < 0.6, t);
    };
    await at(20);
    const base = await pos();
    await page.keyboard.press("ArrowRight");
    expect(await pos()).toBeCloseTo(base + 5, 0);
    await page.keyboard.press("ArrowLeft");
    await page.keyboard.press("ArrowLeft");
    expect(await pos()).toBeCloseTo(base - 5, 0);
    await page.keyboard.press("Shift+ArrowRight");
    expect(await pos()).toBeCloseTo(base + 25, 0);
    await page.keyboard.press("j");
    expect(await pos()).toBeCloseTo(base + 15, 0);
    await page.keyboard.press("l");
    expect(await pos()).toBeCloseTo(base + 25, 0);
    await page.keyboard.press("Home");
    expect(await pos()).toBeLessThan(0.5);
    await page.keyboard.press("End");
    expect(await pos()).toBeGreaterThan(55);
    await page.keyboard.press("Home");

    await page.keyboard.press("m");
    expect((await snap(page)).muted).toBe(true);
    await page.keyboard.press("m");
    expect((await snap(page)).muted).toBe(false);
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    expect((await snap(page)).volume).toBeCloseTo(0.9, 2);
    await page.keyboard.press("ArrowUp");
    expect((await snap(page)).volume).toBeCloseTo(0.95, 2);

    await page.keyboard.press("k"); // play/pause alias
    await waitState(page, "playing");
    await page.keyboard.press("k");
    await waitState(page, "paused");

    await page.keyboard.press("f");
    await waitFor(page, () => window.rvp.snapshot().fullscreen === true);
    await page.keyboard.press("f");
    await waitFor(page, () => window.rvp.snapshot().fullscreen === false);
  });

  test("e) the right-click menu offers every action the keyboard has", async ({ page }) => {
    await load(page, LONG, { play: false });
    await page.mouse.click(640, 300, { button: "right" });
    let s = await snap(page);
    expect(s.menu_open).toBe(true);
    const labels = s.menu.map((m) => m.label);
    for (const want of ["Open file…", "Play", "Seek", "Speed", "Volume", "Audio track", "Subtitles", "Fullscreen"]) {
      expect(labels, `main menu has ${want}`).toContain(want);
    }
    const row = (name) => s.menu.find((m) => m.label === name).rect;
    const center = (r) => [r.x + r.w / 2, r.y + r.h / 2];
    // Submenus open on hover.
    await page.mouse.move(...center(row("Seek")));
    s = await snap(page);
    const sub = s.menu.map((m) => m.label);
    for (const want of ["Back 5 s", "Forward 5 s", "Back 10 s", "Forward 10 s", "Back 30 s", "Forward 30 s", "To start", "To end"]) {
      expect(sub, `seek submenu has ${want}`).toContain(want);
    }
    await page.mouse.move(...center(s.menu.find((m) => m.label === "Speed").rect));
    s = await snap(page);
    expect(s.menu.map((m) => m.label)).toContain("Normal speed");
    // Run an item from a submenu.
    const two = s.menu.find((m) => m.label === "2×");
    await page.mouse.move(...center(two.rect));
    await page.mouse.click(...center(two.rect));
    s = await snap(page);
    expect(s.menu_open).toBe(false);
    expect(s.rate).toBe(2);
    // Escape closes a menu; so does a click elsewhere.
    await page.mouse.click(640, 300, { button: "right" });
    expect((await snap(page)).menu_open).toBe(true);
    await page.keyboard.press("Escape");
    expect((await snap(page)).menu_open).toBe(false);
    // Keyboard-only: the menu key, arrows and Enter.
    await page.keyboard.press("ContextMenu");
    expect((await snap(page)).menu_open).toBe(true);
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter"); // Seek > Forward 5 s
    const s2 = await snap(page);
    expect(s2.menu_open).toBe(false);
    expect(s2.position_us).toBeGreaterThan(4_900_000);
  });

  test("speed: the 2x entry runs twice as fast and the speed button shows it", async ({ page }) => {
    await load(page);
    await page.keyboard.press("]"); // 1.25
    await page.keyboard.press("]"); // 1.5
    await page.keyboard.press("]"); // 2
    expect((await snap(page)).rate).toBe(2);
    await waitState(page, "playing");
    await playedFor(page, 2_000_000); // the time stretcher has warmed up
    const run = await rateAgainstDevice(page, 5_000_000);
    expect(run.ratio, `position moved ${(run.moved / 1000).toFixed(0)} ms in ${(run.device / 1000).toFixed(0)} ms of device time`).toBeGreaterThan(1.85);
    expect(run.ratio).toBeLessThan(2.15);
    await page.keyboard.press("\\");
    expect((await snap(page)).rate).toBe(1);
  });

  test("drag and drop opens a file", async ({ page }) => {
    await page.goto("/");
    await waitFor(page, () => window.rvp && window.rvp.ready);
    const b64 = fs.readFileSync(SHORT).toString("base64");
    await page.evaluate(async (b64) => {
      const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      const dt = new DataTransfer();
      dt.items.add(new File([bytes], "dropped.webm", { type: "video/webm" }));
      window.dispatchEvent(new DragEvent("dragenter", { dataTransfer: dt, bubbles: true, cancelable: true }));
      window.dispatchEvent(new DragEvent("dragover", { dataTransfer: dt, bubbles: true, cancelable: true }));
    }, b64);
    // While a file hovers, the empty screen invites the drop.
    await frames(page, 3);
    await page.evaluate(async (b64) => {
      const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      const dt = new DataTransfer();
      dt.items.add(new File([bytes], "dropped.webm", { type: "video/webm" }));
      window.dispatchEvent(new DragEvent("drop", { dataTransfer: dt, bubbles: true, cancelable: true }));
    }, b64);
    await waitState(page, "playing");
    const s = await snap(page);
    expect(s.title).toBe("dropped.webm");
    expect(s.duration_us).toBeGreaterThan(5_000_000);
  });

  test("open: the Open button and the O key show the file picker", async ({ page }) => {
    await page.goto("/");
    await waitFor(page, () => window.rvp && window.rvp.ready);
    const s = await snap(page);
    const b = s.buttons.Welcome;
    const [chooser] = await Promise.all([page.waitForEvent("filechooser"), page.mouse.click(b.x + b.w / 2, b.y + b.h / 2)]);
    await chooser.setFiles(SHORT);
    await waitState(page, "playing");
    expect((await snap(page)).title).toBe("av1_opus.webm");
    // While playing, the keyboard asks again.
    const [again] = await Promise.all([page.waitForEvent("filechooser"), page.keyboard.press("o")]);
    await again.setFiles(LONG);
    await waitFor(page, () => window.rvp.snapshot().title === "av1_opus_60s.webm");
    // And so does the transport bar's Open button.
    const bar = (await snap(page)).buttons.Open;
    await page.mouse.move(bar.x + 5, bar.y + 5);
    const [third] = await Promise.all([page.waitForEvent("filechooser"), page.mouse.click(bar.x + bar.w / 2, bar.y + bar.h / 2)]);
    await third.setFiles(SHORT);
    await waitFor(page, () => window.rvp.snapshot().title === "av1_opus.webm");
  });

  test("pointer: click toggles playback, the wheel sets the volume, sliders drag", async ({ page }) => {
    await load(page);
    const gap = () => page.waitForTimeout(450); // two clicks closer together are a double click
    await page.mouse.click(640, 300);
    await waitState(page, "paused");
    await gap();
    await page.mouse.click(640, 300);
    await waitState(page, "playing");
    await gap();
    // Middle click also toggles.
    await page.mouse.click(640, 300, { button: "middle" });
    await waitState(page, "paused");
    await page.mouse.click(640, 300, { button: "middle" });
    await waitState(page, "playing");
    await gap();
    // Double click: fullscreen (and the first click's toggle is undone by the second).
    await page.mouse.dblclick(640, 300);
    await waitFor(page, () => window.rvp.snapshot().fullscreen === true);
    // Entering fullscreen re-composes at the new size, which can briefly stall a busy machine ("buffering").
    await waitState(page, "playing");
    await page.keyboard.press("Escape");
    await waitFor(page, () => window.rvp.snapshot().fullscreen === false);

    // Wheel over the picture changes the volume, 5% per notch.
    await page.mouse.move(640, 300);
    await page.mouse.wheel(0, 100);
    await waitFor(page, () => Math.abs(window.rvp.snapshot().volume - 0.9) < 0.001);
    await page.mouse.wheel(0, -100);
    await waitFor(page, () => Math.abs(window.rvp.snapshot().volume - 1.0) < 0.001);

    // Volume slider: click at the middle.
    let s = await snap(page);
    const v = s.volume_slider;
    await page.mouse.move(v.x + 10, v.y + 10);
    await page.mouse.click(v.x + v.w * 0.5, v.y + v.h / 2);
    s = await snap(page);
    expect(s.volume).toBeGreaterThan(0.4);
    expect(s.volume).toBeLessThan(0.6);
    // The mute button.
    const mute = s.buttons.Mute;
    await page.mouse.click(mute.x + mute.w / 2, mute.y + mute.h / 2);
    expect((await snap(page)).muted).toBe(true);
    await page.mouse.click(mute.x + mute.w / 2, mute.y + mute.h / 2);
    expect((await snap(page)).muted).toBe(false);

    // Seek bar: press at 20%, drag to 70%, release; the position follows the pointer.
    s = await snap(page);
    const y = s.seek.y + s.seek.h / 2;
    await page.mouse.move(s.seek.x + s.seek.w * 0.2, y);
    await page.mouse.down();
    await page.mouse.move(s.seek.x + s.seek.w * 0.45, y, { steps: 5 });
    await page.mouse.move(s.seek.x + s.seek.w * 0.7, y, { steps: 5 });
    await page.mouse.up();
    const want = 0.7 * (s.duration_us / 1e6);
    await waitFor(page, (t) => Math.abs(window.rvp.snapshot().position_us / 1e6 - t) < 2, want);
  });

  test("controls auto-hide during playback and return on pointer motion", async ({ page }) => {
    await load(page);
    await page.mouse.move(600, 300);
    await page.mouse.move(640, 320);
    expect((await snap(page)).controls_visible).toBe(true);
    await waitFor(page, () => window.rvp.snapshot().controls_visible === false, null, 20_000); // 2.5 s idle plus the fade
    await waitFor(page, () => window.rvp.snapshot().controls_opacity === 0);
    const hidden = await snap(page);
    expect(hidden.controls_visible).toBe(false);
    expect(await page.evaluate(() => document.getElementById("screen").style.cursor)).toBe("none");
    await page.mouse.move(700, 330);
    await waitFor(page, () => window.rvp.snapshot().controls_visible === true);
    // Paused controls stay.
    await page.keyboard.press("Space");
    await waitState(page, "paused");
    // Longer than the idle time: real time is what this is about (it can only be longer on a busy machine).
    await page.waitForTimeout(3600);
    expect((await snap(page)).controls_visible).toBe(true);
  });

  test("prefers-reduced-motion: the controls vanish without a fade", async ({ browser }) => {
    const ctx = await browser.newContext({ reducedMotion: "reduce", viewport: { width: 1280, height: 720 } });
    const page = await ctx.newPage();
    await load(page);
    // Record the opacity on every frame until the controls are gone: it only ever takes its two end values (on a slow machine they
    // may already have gone when the recording starts, so the other end is not required to be seen).
    await page.evaluate(() => {
      window.__opacities = new Set();
      const look = () => {
        window.__opacities.add(window.rvp.snapshot().controls_opacity);
        requestAnimationFrame(look);
      };
      look();
    });
    await waitFor(page, () => window.rvp.snapshot().controls_opacity === 0, null, 20_000);
    await frames(page, 10);
    const seen = await page.evaluate(() => [...window.__opacities]);
    expect(seen.length).toBeGreaterThan(0);
    expect(seen.filter((v) => v !== 0 && v !== 1), "no value between the two ends").toEqual([]);
    expect(seen).toContain(0);
    await ctx.close();
  });

  test("f) the paused screen matches the committed golden", async ({ page }) => {
    await load(page, LONG, { play: false });
    const s = await snap(page);
    // A fixed position makes the picture deterministic: seek to 25% (15.0 s) and park the pointer.
    await page.mouse.click(s.seek.x + s.seek.w * 0.25, s.seek.y + 2);
    await waitFor(page, () => Math.abs(window.rvp.snapshot().position_us / 1e6 - 15) < 0.1);
    await page.mouse.move(640, 250);
    // The picture for the new position is there and nothing moves any more (hover and tooltip timers, the fade of the controls).
    await waitFor(page, () => window.rvp.snapshot().video.presented > 0 && window.rvp.snapshot().state === "paused");
    await waitFor(page, () => window.rvp.snapshot().toast === null, null, 30_000);
    await settled(
      page,
      () => {
        const q = window.rvp.snapshot();
        return [q.position_us, q.video.presented, q.controls_opacity, q.menu_open, q.toast];
      },
      { n: 8, gap: 3 },
    );
    const png = await page.evaluate(() => window.rvp.png());
    if (process.env.UPDATE_GOLDEN || !fs.existsSync(GOLDEN)) {
      fs.mkdirSync(path.dirname(GOLDEN), { recursive: true });
      fs.writeFileSync(GOLDEN, Buffer.from(png.split(",")[1], "base64"));
      console.log(`wrote ${GOLDEN}`);
    }
    const golden = "data:image/png;base64," + fs.readFileSync(GOLDEN).toString("base64");
    const d = await page.evaluate((g) => window.rvp.diffAgainst(g), golden);
    expect(d.sizeMismatch, "golden size").toBeUndefined();
    expect(d.mean, `mean error ${d.mean}`).toBeLessThan(1.5);
    expect(d.badFraction, `${(d.badFraction * 100).toFixed(2)}% of pixels differ`).toBeLessThan(0.01);
  });
});

test.describe("crash recovery", () => {
  for (const what of ["main", "decoder"]) {
    test(`a crash of the ${what === "main" ? "player" : "video decoder"} restarts the player and playback goes on`, async ({ page }) => {
      const logs = [];
      page.on("console", (m) => m.type() === "error" && logs.push(m.text()));
      page.on("pageerror", (e) => logs.push(String(e)));
      await load(page, H264_AAC);
      await playedFor(page, 1_000_000);
      expect(await page.evaluate(() => window.rvp.recoveries())).toBe(0);
      await page.evaluate((w) => window.rvp.debugCrash(w), what);
      // The page throws the damaged instance away, starts a new one and opens the file again.
      await waitFor(page, () => window.rvp.recoveries() === 1, null, 30_000);
      await waitState(page, "playing");
      await waitFor(page, () => (window.rvp.snapshot().video || { presented: 0 }).presented > 3, null, 30_000);
      const before = await picture(page);
      expect(diff(before, await pictureChange(page, before, 0.005)), "the picture moves again").toBeGreaterThan(0.005);
      expect(logs.some((l) => /crash|panick|unreachable/i.test(l)), "the crash was reported").toBe(true);
    });
  }
});
