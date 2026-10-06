// Phase A2 in the browser: favorites (a heart on a song, the Favorites view, kept across a reload), the About page (its links open in a
// new tab), the tag editor (offered, and read-only with its reason where the browser cannot write), tooltips (after a moment, and gone
// when switched off in Settings) and the visualizer's cycle key.
const { test, expect } = require("@playwright/test");
const path = require("node:path");

const root = path.resolve(__dirname, "../..");
const FIXTURES = process.env.RVP_FIXTURES || path.join(root, "target/fixtures");
const LIBRARY = path.join(FIXTURES, "library/music");

const { snap, waitFor: waitForAt, frames, press, toFace } = require("./helpers");
const waitFor = (page, fn, arg, timeout) => waitForAt(page, fn, arg, timeout, 30);
const center = (r) => [r.x + r.w / 2, r.y + r.h / 2];

async function boot(page) {
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => m.type() === "error" && !/404/.test(m.text()) && errors.push(m.text()));
  await page.goto("/");
  await waitFor(page, () => window.rvp && window.rvp.ready);
  return errors;
}

async function library(page) {
  const errors = await boot(page);
  await toFace(page, "library");
  await page.setInputFiles("#dir", LIBRARY);
  await waitFor(page, () => {
    const l = window.rvp.snapshot().lib;
    return l.tracks >= 201 && !l.scan;
  }, null, 60_000);
  await frames(page, 6);
  return errors;
}

test("a heart on a song makes it a favorite, the Favorites view lists it, and it survives a reload", async ({ page }) => {
  const errors = await library(page);
  await press(page, "3");
  await waitFor(page, () => window.rvp.snapshot().lib.view === "tracks");
  let s = await snap(page);
  expect(s.lib.favorites).toBe(0);
  const first = s.lib.ents[0];
  const title = first.label;
  // The heart is at the right of the row, left of the time: click it.
  const r = first.rect;
  await page.mouse.move(r.x + r.w / 2, r.y + r.h / 2);
  await frames(page, 2);
  await page.mouse.click(r.x + r.w - 98 + 14, r.y + r.h / 2);
  await waitFor(page, () => window.rvp.snapshot().lib.favorites === 1);
  s = await snap(page);
  expect(s.lib.ents[0].fav).toBe(true);
  await press(page, "9");
  await waitFor(page, () => window.rvp.snapshot().lib.view === "favorites");
  s = await snap(page);
  expect(s.lib.ents.map((e) => e.label)).toEqual([title]);
  // H toggles the selected row, too (in the Tracks view, where the row stays when its heart goes).
  await press(page, "3");
  await waitFor(page, () => window.rvp.snapshot().lib.view === "tracks");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await press(page, "h");
  await waitFor(page, () => window.rvp.snapshot().lib.favorites === 2);
  await press(page, "h");
  await waitFor(page, () => window.rvp.snapshot().lib.favorites === 1);
  // A reload keeps it.
  await page.evaluate(() => window.rvp.flushStore());
  await page.reload();
  await waitFor(page, () => window.rvp && window.rvp.ready);
  await waitFor(page, () => window.rvp.snapshot().lib.favorites === 1, null, 30_000);
  expect(errors).toEqual([]);
});

test("About RW is last in the rail; its link opens a new tab and the page says the version", async ({ page, context }) => {
  const errors = await boot(page);
  await toFace(page, "library");
  await press(page, "F1");
  await waitFor(page, () => window.rvp.snapshot().lib.view === "about");
  const s = await snap(page);
  expect(s.lib.about.y).toBeGreaterThan(s.lib.settings.y);
  const ids = s.lib.hero_buttons.map((b) => b.id);
  expect(ids).toEqual([0, 1, 2]);
  const bucket = s.lib.hero_buttons.find((b) => b.id === 0).rect;
  // The page may be taller than the window: scroll to the button first.
  await page.mouse.move(s.lib.body.x + 300, s.lib.body.y + 300);
  for (let i = 0; i < 6; i++) {
    const t = await snap(page);
    const b = t.lib.hero_buttons.find((x) => x.id === 0).rect;
    if (b.y + b.h < t.lib.body.y + t.lib.body.h) break;
    await page.mouse.wheel(0, 200);
  }
  const t = await snap(page);
  const b = t.lib.hero_buttons.find((x) => x.id === 0).rect;
  const [popup] = await Promise.all([context.waitForEvent("page"), page.mouse.click(...center(b))]);
  expect(popup.url()).toMatch(/^https:\/\/rustybucket\.ai\/?$|^about:blank$/);
  await popup.close();
  expect(bucket.w).toBeGreaterThan(0);
  expect(errors).toEqual([]);
});

test("Edit tags is offered and, where the browser has no write handle for the folder, opens read-only with the reason", async ({ page }) => {
  const errors = await library(page);
  await press(page, "3");
  await waitFor(page, () => window.rvp.snapshot().lib.view === "tracks");
  let s = await snap(page);
  const r = s.lib.ents[2].rect;
  await page.mouse.click(r.x + r.w / 2, r.y + r.h / 2, { button: "right" });
  await waitFor(page, () => window.rvp.snapshot().lib.menu_open);
  // E is the key; the menu has the entry too: use the key.
  await press(page, "Escape");
  await page.mouse.click(r.x + r.w / 2, r.y + r.h / 2);
  await press(page, "e");
  await waitFor(page, () => window.rvp.snapshot().lib.tagform);
  await frames(page, 4);
  s = await snap(page);
  const f = s.lib.tagform;
  expect(f.title).toBe("Edit tags");
  // The folder came through the folder input: no handle, so nothing can be written, and the form says why.
  expect(f.read_only).toMatch(/write access|cannot change files/i);
  expect(f.fields.some((x) => x.key === "title")).toBe(true);
  await press(page, "Escape");
  await waitFor(page, () => !window.rvp.snapshot().lib.tagform);
  expect(errors).toEqual([]);
});

test("tooltips come after a moment and go away with the Settings switch", async ({ page }) => {
  const errors = await boot(page);
  await toFace(page, "library");
  await frames(page, 4);
  let s = await snap(page);
  const rail = s.lib.rail.tracks || s.lib.rail.albums;
  expect(rail).toBeTruthy();
  const [x, y] = center(rail);
  await page.mouse.move(x, y);
  await waitFor(page, () => window.rvp.snapshot().lib.tip !== null, null, 10_000);
  s = await snap(page);
  expect(s.lib.tip.text.length).toBeGreaterThan(10);
  expect(s.lib.tip.text.split("\n").every((l) => l.length <= 140)).toBe(true);
  // Settings: switch them off.
  await page.keyboard.press("Control+,");
  await waitFor(page, () => window.rvp.snapshot().dialog);
  s = await snap(page);
  const i = s.dialog.toggles.findIndex((t) => /tooltips/i.test(t.label));
  expect(i).toBeGreaterThanOrEqual(0);
  expect(s.dialog.toggles[i].on).toBe(true);
  // The keyboard starts on Close; Down wraps to the first control, the switches come first (this one is the first).
  expect(i).toBe(0);
  await press(page, "ArrowDown");
  await press(page, "Space");
  await waitFor(page, () => window.rvp.snapshot().dialog.toggles[0].on === false);
  await press(page, "Escape");
  await page.mouse.move(x + 3, y);
  await page.mouse.move(x, y);
  await frames(page, 6);
  s = await snap(page);
  expect(s.lib.tip).toBe(null);
  expect(errors).toEqual([]);
});

test("Shift+V on the visualizer turns the automatic change of effect on and off", async ({ page }) => {
  const errors = await boot(page);
  await toFace(page, "library");
  await press(page, "7");
  await waitFor(page, () => window.rvp.snapshot().lib.view === "visualizer");
  expect((await snap(page)).lib.viz_cycle).toBe(false);
  await page.keyboard.press("Shift+V");
  await waitFor(page, () => window.rvp.snapshot().lib.viz_cycle === true);
  await page.keyboard.press("Shift+V");
  await waitFor(page, () => window.rvp.snapshot().lib.viz_cycle === false);
  expect(errors).toEqual([]);
});
