// The side menu (the rail) collapses to icons and comes back: the button at its top, Ctrl+B, the right-click menu; everything stays
// reachable, and the choice is kept across a reload.
const { test, expect } = require("@playwright/test");
const { snap, waitFor, frames, press } = require("./helpers");

const center = (r) => [r.x + r.w / 2, r.y + r.h / 2];

async function boot(page) {
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => m.type() === "error" && !/404/.test(m.text()) && errors.push(m.text()));
  await page.goto("/");
  await waitFor(page, () => window.rvp && window.rvp.ready);
  await frames(page, 3);
  return errors;
}

const railWidth = async (page) => (await snap(page)).lib.rail_box.w;

test("the rail collapses with the button, Ctrl+B and the menu, keeps every view reachable and remembers the choice", async ({ page }) => {
  const errors = await boot(page);
  let s = await snap(page);
  expect(s.lib.rail_collapsed).toBe(false);
  expect(s.lib.rail_toggle).not.toBeNull();
  const full = s.lib.rail_box.w;
  const views = Object.keys(s.lib.rail).length;
  expect(views).toBeGreaterThan(8);
  // The button.
  await page.mouse.click(...center(s.lib.rail_toggle));
  await waitFor(page, () => window.rvp.snapshot().lib.rail_collapsed === true);
  await waitFor(page, (f) => window.rvp.snapshot().lib.rail_box.w < f * 0.5, full);
  s = await snap(page);
  expect(Object.keys(s.lib.rail).length).toBe(views); // every view is still in the rail, as an icon
  // An icon still goes to its view.
  await page.mouse.click(...center(s.lib.rail.queue));
  await waitFor(page, () => window.rvp.snapshot().lib.view === "queue");
  // A reload keeps it.
  await page.evaluate(() => window.rvp.flushStore());
  await page.reload();
  await waitFor(page, () => window.rvp && window.rvp.ready);
  await waitFor(page, () => window.rvp.snapshot().lib && window.rvp.snapshot().lib.rail_collapsed === true, null, 30_000);
  // Ctrl+B expands it again; right-click then offers to collapse.
  await press(page, "Control+b");
  await waitFor(page, () => window.rvp.snapshot().lib.rail_collapsed === false);
  await waitFor(page, (f) => Math.abs(window.rvp.snapshot().lib.rail_box.w - f) < 2, full);
  s = await snap(page);
  const gap = [s.lib.rail_box.x + s.lib.rail_box.w / 2, (s.lib.rail.visualizer.y + s.lib.rail.visualizer.h + s.lib.add_folder.y) / 2];
  await page.mouse.click(gap[0], gap[1], { button: "right" });
  await waitFor(page, () => window.rvp.snapshot().menu_open);
  s = await snap(page);
  const row = s.menu.find((m) => m.label === "Collapse sidebar");
  expect(row).toBeTruthy();
  await page.mouse.click(...center(row.rect));
  await waitFor(page, () => window.rvp.snapshot().lib.rail_collapsed === true);
  expect(errors).toEqual([]);
});
