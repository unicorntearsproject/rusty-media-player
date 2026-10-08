// The Help page and the keys around it, in the browser: H and ? open it (Esc, H or ? close it), it scrolls and fits a phone, Ctrl+F is
// the heart (and the browser's find stays out of the way while the app uses it), and Ctrl+Q is left to the browser (the app offers no
// quit in a tab).
const { test, expect } = require("@playwright/test");
const { waitFor, frames, snap, press } = require("./helpers");

async function boot(page) {
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => m.type() === "error" && !/404/.test(m.text()) && errors.push(m.text()));
  await page.goto("/");
  await waitFor(page, () => window.rvp && window.rvp.ready);
  await frames(page, 3);
  return errors;
}

const help = async (page) => (await snap(page)).help;

test("H and ? open the help page, Page Down scrolls it, Esc, H and ? close it", async ({ page }) => {
  const errors = await boot(page);
  for (const [open, close] of [["h", "Escape"], ["?", "h"], ["h", "?"]]) {
    expect(await help(page)).toBeNull();
    await press(page, open);
    const h = await help(page);
    expect(h).not.toBeNull();
    expect(h.card.w).toBeGreaterThan(300);
    expect(h.scroll).toBe(0);
    // Keys other than the closing ones go to the page, not to the player behind it.
    await press(page, "PageDown");
    expect((await help(page)).scroll).toBeGreaterThan(50);
    await press(page, "Home");
    expect((await help(page)).scroll).toBe(0);
    await press(page, close);
    expect(await help(page)).toBeNull();
  }
  expect(errors).toEqual([]);
});

test("the help page fits a phone and a drag scrolls it", async ({ browser }) => {
  const context = await browser.newContext({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true, deviceScaleFactor: 3 });
  const page = await context.newPage();
  await boot(page);
  await press(page, "?");
  const h = await help(page);
  expect(h).not.toBeNull();
  // The snapshot is in device pixels.
  expect(h.card.x).toBeGreaterThanOrEqual(0);
  expect(h.card.x + h.card.w).toBeLessThanOrEqual(390 * 3);
  expect(h.card.y + h.card.h).toBeLessThanOrEqual(844 * 3);
  const over = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(over).toBeLessThanOrEqual(0);
  // Wheel scrolling works as well as a finger.
  await page.mouse.move(195, 400);
  await page.mouse.wheel(0, 300);
  await waitFor(page, () => window.rvp.snapshot().help && window.rvp.snapshot().help.scroll > 50);
  await context.close();
});

test("Ctrl+F is the heart and keeps the browser's find away; Ctrl+Q is not taken in a tab", async ({ page }) => {
  const errors = await boot(page);
  // Dispatch the keys ourselves to see whether the page kept the browser from acting on them.
  const prevented = (key) =>
    page.evaluate((key) => {
      const e = new KeyboardEvent("keydown", { key, ctrlKey: true, bubbles: true, cancelable: true });
      window.dispatchEvent(e);
      return e.defaultPrevented;
    }, key);
  expect(await prevented("f")).toBe(true);
  expect(await prevented("q")).toBe(false);
  expect((await snap(page)).app.quit).toBe(false);
  // Ctrl+F did not open the search view any more.
  expect((await snap(page)).lib.view).not.toBe("search");
  await press(page, "/");
  await waitFor(page, () => window.rvp.snapshot().lib.view === "search");
  expect(errors).toEqual([]);
});
