// The installable web app (M11): a valid manifest with real icons, a service worker that precaches the app, an app that starts and plays a
// locally opened file with the network switched off, a favicon, and the update flow (a new service worker offers "Update available").
const { test, expect } = require("@playwright/test");
const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "../..");
const FIXTURES = process.env.RVP_FIXTURES || path.join(root, "target/fixtures");
const VIDEO = path.join(FIXTURES, "h264_aac.mp4");

const waitFor = (page, fn, arg, timeout) => page.waitForFunction(fn, arg, { timeout: timeout || 20_000, polling: 30 });
const ready = (page) => waitFor(page, () => window.rvp && window.rvp.ready);

test("the manifest is valid, its icons exist and the page links the favicon", async ({ page }) => {
  await page.goto("/");
  await ready(page);
  const href = await page.locator('link[rel="manifest"]').getAttribute("href");
  expect(href).toBeTruthy();
  const res = await page.request.get(new URL(href, page.url()).toString());
  expect(res.ok()).toBe(true);
  expect(res.headers()["content-type"]).toContain("manifest+json");
  const m = await res.json();
  expect(m.name).toBe("Rusty Video Player");
  expect(m.short_name.length).toBeLessThanOrEqual(12);
  expect(m.display).toBe("standalone");
  expect(m.start_url).toBeTruthy();
  expect(m.scope).toBeTruthy();
  expect(m.id).toBeTruthy();
  expect(m.background_color).toMatch(/^#[0-9a-f]{6}$/i);
  expect(m.theme_color).toMatch(/^#[0-9a-f]{6}$/i);
  // Installability wants a 192 and a 512 icon; maskable ones are separate files.
  const has = (size, purpose) => m.icons.some((i) => i.sizes === `${size}x${size}` && i.purpose === purpose && i.type === "image/png");
  for (const size of [192, 512]) {
    expect(has(size, "any"), `${size} any`).toBe(true);
    expect(has(size, "maskable"), `${size} maskable`).toBe(true);
  }
  for (const icon of m.icons) {
    const r = await page.request.get(new URL(icon.src, new URL(href, page.url())).toString());
    expect(r.ok(), icon.src).toBe(true);
    expect(r.headers()["content-type"]).toMatch(/^image\//);
    if (icon.type === "image/png") {
      const buf = await r.body();
      expect(buf.subarray(1, 4).toString()).toBe("PNG");
      const [w, h] = [buf.readUInt32BE(16), buf.readUInt32BE(20)];
      expect(`${w}x${h}`, icon.src).toBe(icon.sizes);
    }
  }
  // File handlers and the share target use real media types.
  expect(m.file_handlers[0].accept["video/mp4"]).toContain(".mp4");
  expect(m.share_target.method).toBe("POST");
  expect(m.share_target.params.files[0].name).toBe("files");
  // The tab icon and the Apple touch icon.
  for (const sel of ['link[rel="icon"][type="image/svg+xml"]', 'link[rel="apple-touch-icon"]']) {
    const h = await page.locator(sel).first().getAttribute("href");
    expect((await page.request.get(new URL(h, page.url()).toString())).ok(), sel).toBe(true);
  }
  expect((await page.request.get("/favicon.ico")).status()).toBe(404); // the page names its icons; no stray request is needed
});

test("a service worker precaches the app, and with the network off it starts and plays a local file", async ({ page, context }) => {
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto("/");
  await ready(page);
  await page.evaluate(() => navigator.serviceWorker.ready);
  await page.reload();
  await ready(page);
  expect(await page.evaluate(() => !!navigator.serviceWorker.controller), "the page is controlled after a reload").toBe(true);
  // What is in the cache: the shell, the scripts, the wasm and the icons.
  const cached = await page.evaluate(async () => {
    const key = (await caches.keys()).find((k) => k.startsWith("rvp-app-"));
    const c = await caches.open(key);
    return { key, urls: (await c.keys()).map((r) => new URL(r.url).pathname) };
  });
  for (const f of ["/", "/index.html", "/main.js", "/pwa.js", "/audio-worklet.js", "/pkg/rvp.js", "/pkg/rvp_bg.wasm", "/manifest.webmanifest", "/icons/icon-512.png", "/style.css"]) {
    expect(cached.urls, f).toContain(f);
  }
  expect(cached.key).toMatch(/^rvp-app-\d+\.\d+\.\d+/);
  // Offline: reload and play.
  await context.setOffline(true);
  await page.reload();
  await ready(page);
  expect(await page.evaluate(() => navigator.onLine)).toBe(false);
  await page.setInputFiles("#file", VIDEO);
  await waitFor(page, () => window.rvp.snapshot().state === "playing");
  const a = await page.evaluate(() => window.rvp.snapshot().position_us);
  await page.waitForTimeout(1500);
  const b = await page.evaluate(() => window.rvp.snapshot().position_us);
  expect(b - a).toBeGreaterThan(1_000_000);
  expect(errors).toEqual([]);
  await context.setOffline(false);
});

test("a changed service worker offers an update and reloads onto it", async ({ page }) => {
  await page.goto("/");
  await ready(page);
  await page.evaluate(() => navigator.serviceWorker.ready);
  await page.reload();
  await ready(page);
  const before = await page.evaluate(async () => (await caches.keys()).filter((k) => k.startsWith("rvp-app-")));
  expect(before.length).toBe(1);
  // A new release: the served sw.js changes (the test edits the built file and puts it back afterwards).
  const swFile = path.join(root, "target/web/sw.js");
  const original = fs.readFileSync(swFile, "utf8");
  try {
    fs.writeFileSync(swFile, original.replace(/const VERSION = "([^"]*)"/, 'const VERSION = "$1-next"'));
    await page.evaluate(async () => (await navigator.serviceWorker.getRegistration()).update());
  const button = page.locator("#update");
  await expect(button).toBeVisible({ timeout: 15_000 });
  await expect(button).toContainText("Update");
  await Promise.all([page.waitForEvent("load"), button.click()]);
  await ready(page);
  await expect.poll(async () => page.evaluate(async () => (await caches.keys()).filter((k) => k.startsWith("rvp-app-"))), { timeout: 10_000 }).toHaveLength(1);
  const after = await page.evaluate(async () => (await caches.keys()).filter((k) => k.startsWith("rvp-app-")));
  expect(after[0]).toContain("-next");
  } finally {
    fs.writeFileSync(swFile, original);
  }
});
