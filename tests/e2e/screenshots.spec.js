// Regenerates docs/screenshots/*.png: `cargo xtask e2e --screenshots`. Skipped in normal runs.
const { test, expect } = require("@playwright/test");
const path = require("node:path");
const fs = require("node:fs");

const root = path.resolve(__dirname, "../..");
const FIXTURES = process.env.RVP_FIXTURES || path.join(root, "target/fixtures");
const OUT = path.join(root, "docs/screenshots");
const LONG = path.join(FIXTURES, "av1_opus_60s.webm");

test.skip(!process.env.RVP_SCREENSHOTS, "set RVP_SCREENSHOTS=1 (cargo xtask e2e --screenshots)");

const snap = (page) => page.evaluate(() => window.rvp.snapshot());
const waitFor = (page, fn, arg) => page.waitForFunction(fn, arg, { timeout: 15_000, polling: 50 });
const shot = (page, name) => page.screenshot({ path: path.join(OUT, name) });
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
  const s = await snap(page);
  await page.mouse.click(s.seek.x + s.seek.w * at, s.seek.y + 2);
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

  // Audio-only file, a file whose picture we cannot decode yet (H.264), and a broken file.
  await ready(page);
  await openPaused(page, path.join(FIXTURES, "mp3.mkv"), 0.3);
  await shot(page, "11-audio-only.png");
  await ready(page);
  await page.setInputFiles("#file", path.join(FIXTURES, "h264_aac.mp4"));
  await waitFor(page, () => window.rvp.snapshot().state === "playing");
  await page.waitForTimeout(600);
  await shot(page, "12-no-picture-h264.png");
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
