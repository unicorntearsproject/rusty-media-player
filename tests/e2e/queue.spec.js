// The queue view lists what will play, in that order, and "Play next" stacks. A folder of generated tracks is scanned through the page's
// folder input; the queue is read from the snapshot's view rows.
const { test, expect } = require("@playwright/test");
const path = require("node:path");
const { snap, waitFor, frames, press } = require("./helpers");

const root = path.resolve(__dirname, "../..");
const FIXTURES = process.env.RVP_FIXTURES || path.join(root, "target/fixtures");
const LIBRARY = path.join(FIXTURES, "library/music");

async function boot(page) {
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => m.type() === "error" && !/404/.test(m.text()) && errors.push(m.text()));
  await page.goto("/");
  await waitFor(page, () => window.rvp && window.rvp.ready);
  await page.setInputFiles("#dir", LIBRARY);
  await waitFor(page, () => window.rvp.snapshot().lib.tracks >= 201 && !window.rvp.snapshot().lib.scan, null, 60_000);
  await frames(page, 6);
  return errors;
}

const queueLabels = async (page) => {
  await press(page, "5");
  await waitFor(page, () => window.rvp.snapshot().lib.view === "queue");
  await frames(page, 2);
  return (await snap(page)).lib.ents.map((e) => e.label);
};

/** Select the track row `n` (0-based) of the Tracks view and return its title. */
async function selectTrack(page, n) {
  await press(page, "3");
  await waitFor(page, () => window.rvp.snapshot().lib.view === "tracks");
  await page.keyboard.press("Home");
  for (let i = 0; i < n; i++) await page.keyboard.press("ArrowDown");
  await frames(page, 2);
  const s = await snap(page);
  return s.lib.ents[s.lib.selected].label;
}

for (const shuffle of [false, true]) {
  test(`the queue is the order Next plays (shuffle ${shuffle ? "on" : "off"}), and play next stacks in order`, async ({ page }) => {
    const errors = await boot(page);
    await selectTrack(page, 1);
    await press(page, "Enter"); // plays the list from the second row
    await waitFor(page, () => window.rvp.snapshot().state === "playing");
    if (shuffle) {
      await press(page, "z");
      await waitFor(page, () => window.rvp.snapshot().shuffle === true);
    }
    // The generated tracks are a second or two long: pause, so nothing moves on by itself while the queue is read.
    await press(page, "Space");
    await waitFor(page, () => window.rvp.snapshot().state === "paused");
    // Now playing on top, then what comes.
    const shown = await queueLabels(page);
    expect(shown.length).toBeGreaterThan(5);
    expect(shown[0]).toBe((await snap(page)).playlist.find((e) => e.current).label);
    // Next goes down the list, one row each press.
    for (let k = 1; k <= 3; k++) {
      await press(page, "n");
      await waitFor(page, (t) => (window.rvp.snapshot().playlist.find((e) => e.current) || {}).label === t, shown[k]);
    }
    // (Next starts playing again: pause once more.)
    if ((await snap(page)).state === "playing") await press(page, "Space");
    await waitFor(page, () => window.rvp.snapshot().state === "paused");
    // Play next twice, from two different rows: they come in the order they were added, ahead of everything else.
    const a = await selectTrack(page, 3);
    await page.keyboard.press("Control+Enter");
    await waitFor(page, () => /Playing next/.test(window.rvp.snapshot().toast || ""));
    const b = await selectTrack(page, 6);
    await page.keyboard.press("Control+Enter");
    await frames(page, 3);
    const after = await queueLabels(page);
    expect(after[1]).toBe(a);
    expect(after[2]).toBe(b);
    // And Next really plays them in that order.
    await press(page, "n");
    await waitFor(page, (t) => (window.rvp.snapshot().playlist.find((e) => e.current) || {}).label === t, a);
    await press(page, "n");
    await waitFor(page, (t) => (window.rvp.snapshot().playlist.find((e) => e.current) || {}).label === t, b);
    expect(errors).toEqual([]);
  });
}
