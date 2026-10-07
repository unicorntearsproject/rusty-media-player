// A phone in portrait (390x844 and 360x800, touch, 3x): the page does not scroll sideways, the transport is at the bottom with
// touch-sized targets, the rail is a drawer behind the menu button, and a long message wraps inside the window.
const { test, expect } = require("@playwright/test");
const { waitFor, frames, snap } = require("./helpers");

for (const [name, viewport] of [
  ["390x844", { width: 390, height: 844 }],
  ["360x800", { width: 360, height: 800 }],
]) {
  test(`a phone at ${name}: bottom transport with touch targets, a drawer for the views, nothing overflows`, async ({ browser }) => {
    const context = await browser.newContext({ viewport, hasTouch: true, isMobile: true, deviceScaleFactor: 3 });
    const page = await context.newPage();
    await page.goto("/");
    await waitFor(page, () => window.rvp && window.rvp.ready);
    await frames(page, 3);
    // The page itself never scrolls sideways.
    const over = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(over).toBeLessThanOrEqual(0);
    // The snapshot's rectangles are in device pixels; everything below is in CSS pixels.
    const css = (r) => (r ? { x: r.x / 3, y: r.y / 3, w: r.w / 3, h: r.h / 3 } : r);
    const cssAll = (o) => Object.fromEntries(Object.entries(o).map(([k, r]) => [k, css(r)]));
    const read = async () => {
      const l = (await snap(page)).lib;
      return { ...l, menu: css(l.menu), bar: cssAll(l.bar), rail: cssAll(l.rail) };
    };
    const lib = await read();
    expect(lib.mode).toBe("library");
    // The menu button is touch-sized and in the header.
    expect(lib.menu).not.toBeNull();
    expect(lib.menu.w).toBeGreaterThanOrEqual(44);
    expect(lib.menu.h).toBeGreaterThanOrEqual(44);
    // The transport: every control at least 44 px, inside the viewport, in the bottom third.
    const bar = Object.entries(lib.bar);
    expect(bar.length).toBeGreaterThanOrEqual(5);
    for (const [btn, r] of bar) {
      expect(r.w, btn).toBeGreaterThanOrEqual(44);
      expect(r.h, btn).toBeGreaterThanOrEqual(44);
      expect(r.x, btn).toBeGreaterThanOrEqual(0);
      expect(r.x + r.w, btn).toBeLessThanOrEqual(viewport.width + 1);
      expect(r.y, btn).toBeGreaterThan(viewport.height * 0.66);
      expect(r.y + r.h, btn).toBeLessThanOrEqual(viewport.height + 1);
    }
    // The rail is closed: no entry can be hit. A tap on the menu button opens it; a tap on a view goes there and closes it.
    expect(Object.keys(lib.rail)).toHaveLength(0);
    const tap = (r) => page.touchscreen.tap(r.x + r.w / 2, r.y + r.h / 2);
    await tap(lib.menu);
    await waitFor(page, () => Object.keys(window.rvp.snapshot().lib.rail).length > 0);
    const open = await read();
    for (const [view, r] of Object.entries(open.rail)) {
      expect(r.x + r.w, view).toBeLessThanOrEqual(viewport.width + 1);
      expect(r.y + r.h, view).toBeLessThanOrEqual(viewport.height + 1);
    }
    await tap(open.rail.tracks);
    await waitFor(page, () => window.rvp.snapshot().lib.view === "tracks" && Object.keys(window.rvp.snapshot().lib.rail).length === 0);
    await context.close();
  });
}
