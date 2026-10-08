// Regenerates docs/screenshots/*.png: `cargo xtask e2e --screenshots`. Skipped in normal runs.
const { test, expect } = require("@playwright/test");
const path = require("node:path");
const fs = require("node:fs");

const root = path.resolve(__dirname, "../..");
const FIXTURES = process.env.RVP_FIXTURES || path.join(root, "target/fixtures");
const OUT = path.join(root, "docs/screenshots");
const LONG = path.join(FIXTURES, "av1_opus_60s.webm");
const SHOWCASE = path.join(FIXTURES, "showcase/music");

test.skip(!process.env.RVP_SCREENSHOTS, "set RVP_SCREENSHOTS=1 (cargo xtask e2e --screenshots)");

const snap = (page) => page.evaluate(() => window.rvp.snapshot());
const waitFor = (page, fn, arg) => page.waitForFunction(fn, arg, { timeout: 15_000, polling: 50 });
const shot = async (page, name, wait = 0) => {
  if (wait) await page.waitForTimeout(wait);
  await page.screenshot({ path: path.join(OUT, name) });
};
const center = (r) => [r.x + r.w / 2, r.y + r.h / 2];

async function ready(page) {
  await page.goto("/");
  await waitFor(page, () => window.rvp && window.rvp.ready);
}

async function openPaused(page, file, at = 0.25) {
  await page.setInputFiles("#file", file);
  await waitFor(page, () => window.rvp.snapshot().state === "playing");
  await waitFor(page, () => (window.rvp.snapshot().video || { presented: 0 }).presented > 3 || !window.rvp.snapshot().has_video);
  await page.keyboard.press("Space");
  await waitFor(page, () => window.rvp.snapshot().state === "paused");
  await waitFor(page, () => window.rvp.snapshot().controls_opacity > 0.95);
  // The snapshot's rects are in surface pixels; mouse coordinates are CSS pixels (they differ on HiDPI screens).
  const dpr = await page.evaluate(() => window.devicePixelRatio || 1);
  const s = await snap(page);
  await page.mouse.click((s.seek.x + s.seek.w * at) / dpr, (s.seek.y + 2) / dpr);
  await waitFor(page, (t) => Math.abs(window.rvp.snapshot().position_us / 1e6 - t) < 1.5, at * (s.duration_us / 1e6));
  await page.waitForTimeout(700);
}

test.beforeAll(() => fs.mkdirSync(OUT, { recursive: true }));

test("screenshots", async ({ page, browser }) => {
  await ready(page);
  await page.waitForTimeout(300);
  await shot(page, "01-idle.png");
  await page.evaluate(() => {
    const dt = new DataTransfer();
    dt.items.add(new File(["x"], "clip.webm", { type: "video/webm" }));
    window.dispatchEvent(new DragEvent("dragenter", { dataTransfer: dt, bubbles: true, cancelable: true }));
  });
  await page.waitForTimeout(200);
  await shot(page, "02-idle-drop.png");
  await page.evaluate(() => window.dispatchEvent(new DragEvent("dragleave", { bubbles: true })));

  // Playing with the controls up, hovering the seek bar.
  await page.setInputFiles("#file", LONG);
  await waitFor(page, () => (window.rvp.snapshot().video || { presented: 0 }).presented > 3);
  await page.waitForTimeout(2200);
  let s = await snap(page);
  await page.mouse.move(s.seek.x + s.seek.w * 0.62, s.seek.y + 2);
  await page.waitForTimeout(250);
  await shot(page, "03-playing-seek-hover.png");

  // Controls away during playback.
  await page.mouse.move(640, 300);
  await page.waitForTimeout(3500);
  await shot(page, "04-playing-controls-hidden.png");

  // Paused: the big play button, the primary button's hover glow and its tooltip.
  await page.keyboard.press("Space");
  await waitFor(page, () => window.rvp.snapshot().state === "paused");
  await waitFor(page, () => window.rvp.snapshot().controls_opacity > 0.95); // the bar must be visible to be clickable
  s = await snap(page);
  await page.mouse.click(s.seek.x + s.seek.w * 0.25, s.seek.y + 2);
  await page.waitForTimeout(700);
  await page.mouse.move(...center(s.buttons.Play));
  await page.waitForTimeout(900);
  await shot(page, "05-paused-play-hover.png");

  // Context menu with the Seek submenu open.
  await page.mouse.move(640, 250);
  await page.mouse.click(700, 220, { button: "right" });
  s = await snap(page);
  await page.mouse.move(...center(s.menu.find((m) => m.label === "Seek").rect));
  await page.waitForTimeout(200);
  await shot(page, "06-context-menu.png");
  await page.keyboard.press("Escape");

  // Speed popup from the transport bar.
  s = await snap(page);
  await page.mouse.click(...center(s.buttons.Speed));
  await page.waitForTimeout(200);
  s = await snap(page);
  await page.mouse.move(...center(s.menu.find((m) => m.label === "1.5×").rect));
  await page.waitForTimeout(150);
  await shot(page, "07-speed-menu.png");
  await page.mouse.click(...center(s.menu.find((m) => m.label === "1.5×").rect));
  await page.waitForTimeout(300);
  await shot(page, "08-speed-toast.png");

  // Audio and subtitle menu.
  s = await snap(page);
  await page.mouse.click(...center(s.buttons.Tracks));
  await page.waitForTimeout(200);
  await shot(page, "09-tracks-menu.png");
  await page.keyboard.press("Escape");

  // Keyboard focus ring.
  await page.keyboard.press("Tab");
  await page.keyboard.press("Tab");
  await page.waitForTimeout(150);
  await shot(page, "10-keyboard-focus.png");

  // Audio-only file, an H.264 file (our own decoder), and a broken file.
  // A song opened from outside goes to the Library face: the now-playing screen (this one has no cover or tags).
  await ready(page);
  await page.setInputFiles("#file", path.join(FIXTURES, "mp3.mkv"));
  await waitFor(page, () => window.rvp.snapshot().state === "playing" && window.rvp.snapshot().lib.mode === "library");
  await page.keyboard.press("Space");
  await waitFor(page, () => window.rvp.snapshot().state === "paused");
  await page.mouse.move(640, 300);
  await page.waitForTimeout(800);
  await shot(page, "11-audio-only.png");
  await ready(page);
  await openPaused(page, path.join(FIXTURES, "h264_aac.mp4"), 0.4);
  await shot(page, "12-h264-playing.png");
  await ready(page);
  await page.setInputFiles("#file", { name: "broken.mp4", mimeType: "video/mp4", buffer: Buffer.alloc(4096) });
  await waitFor(page, () => window.rvp.snapshot().state === "failed");
  await page.waitForTimeout(300);
  await shot(page, "13-error.png");

  // HiDPI: the same UI at a device pixel ratio of 2.
  const ctx = await browser.newContext({ deviceScaleFactor: 2, viewport: { width: 1280, height: 720 } });
  const hi = await ctx.newPage();
  await ready(hi);
  await openPaused(hi, LONG, 0.4);
  await hi.mouse.move(640, 250);
  await hi.waitForTimeout(500);
  await shot(hi, "14-hidpi-paused.png");
  await ctx.close();
  expect(fs.readdirSync(OUT).length).toBeGreaterThanOrEqual(14);
});

test("library screenshots", async ({ page, browser }) => {
  await ready(page);
  // The first run opens on the Library already; B only when a run starts on the Player.
  if ((await snap(page)).lib.mode !== "library") await page.keyboard.press("b");
  await waitFor(page, () => window.rvp.snapshot().lib.mode === "library");
  await page.waitForTimeout(300);
  await shot(page, "15-library-empty.png");
  await page.setInputFiles("#dir", SHOWCASE);
  await waitFor(page, () => window.rvp.snapshot().lib.tracks >= 40 && !window.rvp.snapshot().lib.scan, null);
  await page.waitForTimeout(2600); // the "library updated" toast goes away
  const ent = async (label, kind) => (await snap(page)).lib.ents.find((e) => e.label === label && (!kind || e.kind === kind));

  // Albums with a card under the pointer (its play button shows).
  let e = await ent("Tears for Tomorrow", "album");
  await page.mouse.move(e.rect.x + e.rect.w / 2, e.rect.y + e.rect.h / 2 - 30);
  await shot(page, "16-library-albums.png", 400);
  // An album and its tracks.
  e = await ent("Neon Rain", "album");
  await page.mouse.click(...center(e.rect));
  await waitFor(page, () => window.rvp.snapshot().lib.detail !== null);
  await page.mouse.move(1180, 440);
  await shot(page, "17-library-album.png", 400);
  // Play it: the row with the equaliser, the bar and now playing.
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await waitFor(page, () => window.rvp.snapshot().state === "playing");
  await page.mouse.move(1180, 440);
  await page.waitForTimeout(2500);
  await shot(page, "18-library-album-playing.png");
  await page.keyboard.press("3");
  await page.mouse.move(1180, 440);
  await shot(page, "19-library-tracks.png", 500);
  await page.keyboard.press("2");
  await page.mouse.move(1180, 440);
  await shot(page, "20-library-artists.png", 500);
  await page.keyboard.press("/");
  await page.keyboard.type("moon");
  await shot(page, "21-library-search.png", 500);
  await page.keyboard.press("Escape");
  // A playlist made from the album's menu.
  await page.keyboard.press("1");
  await waitFor(page, () => window.rvp.snapshot().lib.view === "albums");
  await page.waitForTimeout(200);
  e = await ent("Night Drive FM", "album");
  await page.mouse.click(...center(e.rect), { button: "right" });
  await waitFor(page, () => window.rvp.snapshot().menu_open);
  let s = await snap(page);
  await page.mouse.move(...center(s.menu.find((m) => m.label === "Add to playlist").rect));
  await page.waitForTimeout(250);
  await shot(page, "22-library-context-menu.png", 100);
  s = await snap(page);
  await page.mouse.click(...center(s.menu.find((m) => m.label.startsWith("New playlist")).rect));
  await waitFor(page, () => window.rvp.snapshot().lib.typing);
  await page.keyboard.type("Night drive");
  await shot(page, "23-library-name-prompt.png", 300);
  await page.keyboard.press("Enter");
  await waitFor(page, () => window.rvp.snapshot().lib.playlists === 1);
  await page.keyboard.press("4");
  await page.mouse.move(1180, 440);
  await shot(page, "24-library-playlists.png", 2600);
  await page.keyboard.press("5");
  await page.mouse.move(1180, 440);
  await shot(page, "25-library-queue.png", 500);
  await page.keyboard.press("6");
  await page.mouse.move(1180, 440);
  await shot(page, "26-now-playing.png", 1500);
  // The visualizer: every effect.
  await page.keyboard.press("7");
  await page.mouse.move(640, 250);
  await page.waitForTimeout(3300); // the controls settle, the toast goes
  await page.mouse.move(640, 252);
  await page.waitForTimeout(300);
  const names = ["spectrum", "scope", "tunnel", "starfield", "plasma"];
  for (let i = 0; i < 5; i++) {
    await shot(page, `27-visualizer-${names[i]}.png`, 1500);
    await page.keyboard.press("ArrowRight");
  }
  await page.keyboard.press("Escape");

  // A narrow window.
  const ctx = await browser.newContext({ viewport: { width: 520, height: 760 }, deviceScaleFactor: 1 });
  const narrow = await ctx.newPage();
  await ready(narrow);
  await narrow.keyboard.press("b");
  await narrow.setInputFiles("#dir", SHOWCASE);
  await waitFor(narrow, () => window.rvp.snapshot().lib.tracks >= 40 && !window.rvp.snapshot().lib.scan, null);
  await narrow.waitForTimeout(2600);
  await shot(narrow, "28-library-narrow.png", 300);
  await ctx.close();
  expect(fs.readdirSync(OUT).length).toBeGreaterThanOrEqual(32);
});
