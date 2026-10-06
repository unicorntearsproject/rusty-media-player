// The video library in the browser: a folder of films is scanned through the page's folder input, the Videos view shows posters (frames
// decoded in the background) and plays them on the Player face, the library survives a reload with its posters, a resume marker shows on
// a film that was left half-way, the list layout and the search work.
const { test, expect } = require("@playwright/test");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const root = path.resolve(__dirname, "../..");
const FIXTURES = process.env.RVP_FIXTURES || path.join(root, "target/fixtures");
const { snap, waitFor: waitForAt, frames, press, toFace } = require("./helpers");
const waitFor = (page, fn, arg, timeout) => waitForAt(page, fn, arg, timeout, 30);
const center = (r) => [r.x + r.w / 2, r.y + r.h / 2];

/** A folder named `Films` with three small films in it (copies of the generated fixtures), under the system's temp folder. */
function filmsFolder() {
  const dir = path.join(os.tmpdir(), `rvp-videos-e2e-${process.pid}`, "Films");
  fs.mkdirSync(dir, { recursive: true });
  for (const [from, to] of [
    ["h264_aac.mp4", "Harbour Lights.mp4"],
    ["vp9_vorbis.webm", "Night Train.webm"],
    ["av1_opus_60s.webm", "The Long Wait.webm"],
  ]) {
    fs.copyFileSync(path.join(FIXTURES, from), path.join(dir, to));
  }
  return dir;
}

async function boot(page) {
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => m.type() === "error" && !/404/.test(m.text()) && errors.push(m.text()));
  await page.goto("/");
  await waitFor(page, () => window.rvp && window.rvp.ready);
  return errors;
}

async function addFilms(page) {
  await toFace(page, "library");
  await page.setInputFiles("#dir", filmsFolder());
  // The headers are read first, then a poster frame is decoded for each film in the background.
  await waitFor(page, () => {
    const l = window.rvp.snapshot().lib;
    return l.videos === 3 && l.posters === 3 && !l.scan;
  }, null, 25_000).catch(async (e) => {
    const l = await page.evaluate(() => { const l = window.rvp.snapshot().lib; return { ...l, ents: undefined, rail: undefined }; });
    throw new Error(`scan wait: ${JSON.stringify(l)}\n${e}`);
  });
  await press(page, "8");
  await waitFor(page, () => window.rvp.snapshot().lib.view === "videos");
  await frames(page, 4);
}

const films = (s) => s.lib.ents.filter((e) => e.kind === "video");

test("a folder of films becomes a grid of posters, and a double click plays one on the Player face", async ({ page }) => {
  const errors = await boot(page);
  await addFilms(page);
  let s = await snap(page);
  expect(films(s).map((e) => e.label)).toEqual(["Harbour Lights", "Night Train", "The Long Wait"]);
  expect(s.lib.rail.videos).toBeTruthy();
  // Posters, not placeholders: the card's picture is not one flat colour.
  const r = films(s)[0].rect;
  const px = await page.evaluate(([x, y, w, h]) => window.rvp.sample(x, y, w, h, 6), [Math.round(r.x + 8), Math.round(r.y + 8), Math.round(r.w - 16), Math.round(r.w * 0.5625 - 16)]);
  const distinct = new Set(px.map((v, i) => (i % 3 === 0 ? `${px[i]},${px[i + 1]},${px[i + 2]}` : null)).filter(Boolean));
  expect(distinct.size).toBeGreaterThan(20);
  // Double click: the film plays, on the Player face.
  const [cx, cy] = center(films(s)[1].rect);
  await page.mouse.dblclick(cx, cy - 30);
  await waitFor(page, () => window.rvp.snapshot().state === "playing" && window.rvp.snapshot().lib.mode === "player", null, 30_000);
  s = await snap(page);
  expect(s.title).toMatch(/Night Train/);
  expect(errors).toEqual([]);
});

test("the library with its posters survives a reload, and a film left half-way shows a resume marker", async ({ page }) => {
  const errors = await boot(page);
  await addFilms(page);
  let s = await snap(page);
  // Play The Long Wait (60 s), seek to the middle, and leave.
  const [cx, cy] = center(films(s)[2].rect);
  await page.mouse.dblclick(cx, cy - 30);
  await waitFor(page, () => window.rvp.snapshot().state === "playing", null, 30_000);
  s = await snap(page);
  await page.mouse.click(s.seek.x + s.seek.w * 0.5, s.seek.y + 2);
  await waitFor(page, () => Math.abs(window.rvp.snapshot().position_us / 1e6 - 30) < 3);
  await frames(page, 3);
  await page.evaluate(() => window.rvp.saveState());
  await page.evaluate(() => window.rvp.flushStore());
  await page.reload();
  await waitFor(page, () => window.rvp && window.rvp.ready);
  // The index and the posters come back from the store; the folder has to be allowed again only in a browser that forgets handles, and
  // the input-based listing here is re-given (as a returning user would re-pick the folder).
  await toFace(page, "library");
  await waitFor(page, () => window.rvp.snapshot().lib.videos === 3 && window.rvp.snapshot().lib.posters === 3);
  await press(page, "8");
  await waitFor(page, () => window.rvp.snapshot().lib.view === "videos");
  await frames(page, 4);
  await page.setInputFiles("#dir", filmsFolder());
  await waitFor(page, () => !window.rvp.snapshot().lib.scan && window.rvp.snapshot().lib.videos === 3, null, 60_000);
  await waitFor(page, () => window.rvp.snapshot().lib.ents.filter((e) => e.kind === "video").length === 3, null, 10_000);
  s = await snap(page);
  const long = films(s).find((e) => e.label === "The Long Wait");
  expect(long.resume).toBeGreaterThan(0.35);
  expect(long.resume).toBeLessThan(0.65);
  expect(films(s).filter((e) => e.resume).length).toBe(1);
  expect(errors).toEqual([]);
});

test("the list layout and the search find films by name", async ({ page }) => {
  const errors = await boot(page);
  await addFilms(page);
  let s = await snap(page);
  // The header's List button turns the posters into rows (one to a row), the same button brings the posters back.
  const button = (s, label) => s.lib.header_buttons.find((b) => b.label === label);
  await page.mouse.click(...center(button(s, "List").rect));
  await waitFor(page, () => window.rvp.snapshot().lib.header_buttons.some((b) => b.label === "Posters"));
  await frames(page, 3);
  s = await snap(page);
  const xs = new Set(films(s).map((e) => Math.round(e.rect.x)));
  expect(xs.size).toBe(1);
  expect(films(s).length).toBe(3);
  await press(page, "/");
  await page.keyboard.type("night");
  await waitFor(page, () => window.rvp.snapshot().lib.view === "search");
  await frames(page, 4);
  s = await snap(page);
  expect(films(s).map((e) => e.label)).toEqual(["Night Train"]);
  expect(errors).toEqual([]);
});
