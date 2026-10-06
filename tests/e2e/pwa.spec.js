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
  expect(m.name).toBe("Rusty Wave");
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
  for (const sel of ['link[rel="icon"][type="image/png"]', 'link[rel="apple-touch-icon"]']) {
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
  const info = await (await page.request.get("/build-info.json")).json();
  // The worker keeps only the wasm build this browser can run: the threaded one on an isolated page that has it, else the plain one.
  const isolated = await page.evaluate(() => self.crossOriginIsolated);
  const pkg = isolated && info.threads ? "pkg-mt" : "pkg";
  expect(info.schema).toBe(1);
  expect(info.name).toBe("rusty-wave-web");
  expect(info.commit).toMatch(/^[0-9a-f]{40}$|^unknown$/);
  expect(info.built).toMatch(/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$/);
  expect(info["build-info.json"]).toBeUndefined();
  expect(Object.keys(info.files)).not.toContain("build-info.json");
  const hashed = (n) => "/" + Object.keys(info.files).find((f) => f.replace(/\.[0-9a-f]{8}\./, ".") === n);
  for (const f of ["/", "/index.html", hashed("main.js"), hashed("pwa.js"), hashed("audio-worklet.js"), hashed(`${pkg}/rvp.js`), hashed(`${pkg}/rvp_bg.wasm`), "/manifest.webmanifest", "/icons/icon-512.png", hashed("style.css")]) {
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

// ---- the "Install app" entry ----------------------------------------------------------------------------------------------
// A synthetic `beforeinstallprompt` stands in for the browser's offer (headless Chromium never sends one): the page keeps it, shows
// "Install app" and calls `prompt()` when it is pressed. Without the offer the entry shows how to install from the browser's menu,
// and as an installed app (standalone display mode) there is no entry at all.

/** Dispatch what Chromium sends when the app can be installed; `window.__installPrompts` counts the calls of `prompt()`. */
const offerInstall = (page, outcome = "accepted") =>
  page.evaluate((outcome) => {
    window.__installPrompts = 0;
    const e = new Event("beforeinstallprompt", { cancelable: true });
    e.platforms = ["web"];
    e.prompt = () => { window.__installPrompts++; return Promise.resolve(); };
    e.userChoice = Promise.resolve({ outcome, platform: "web" });
    window.dispatchEvent(e);
    return e.defaultPrevented;
  }, outcome);

test("the install button appears with the browser's offer and calls prompt()", async ({ page }) => {
  await page.goto("/");
  await ready(page);
  expect((await page.evaluate(() => window.rvp.installState())).state).not.toBe("installed");
  expect(await offerInstall(page)).toBe(true); // kept for later, so the browser shows no banner of its own
  const button = page.locator("#install");
  await expect(button).toBeVisible();
  await expect(button).toHaveText("Install app");
  expect((await page.evaluate(() => window.rvp.installState())).state).toBe("available");
  await button.click();
  await expect(button).toBeHidden();
  expect(await page.evaluate(() => window.__installPrompts)).toBe(1);
  await expect(page.locator("#install-hint")).toBeHidden();
  // `appinstalled` keeps it away, with a note for screen readers.
  await page.evaluate(() => window.dispatchEvent(new Event("appinstalled")));
  await expect(page.locator("#install")).toBeHidden();
  await expect(page.locator("#status")).toHaveText("Rusty Wave was installed");
  expect((await page.evaluate(() => window.rvp.installState())).state).toBe("installed");
});

test("window.rvp.install() asks the browser, and a declined offer is not pressed on the person again", async ({ page }) => {
  await page.goto("/");
  await ready(page);
  await offerInstall(page, "dismissed");
  expect(await page.evaluate(() => window.rvp.install())).toBe("dismissed");
  expect(await page.evaluate(() => window.__installPrompts)).toBe(1);
  await expect(page.locator("#install")).toBeHidden();
  expect((await page.evaluate(() => window.rvp.installState())).dismissed).toBe(true);
  // After a reload the offer arrives again (as a browser would send it) and the button stays away.
  await page.reload();
  await ready(page);
  await offerInstall(page);
  await expect(page.locator("#install")).toBeHidden();
  // Still available for a settings entry to call.
  expect(await page.evaluate(() => window.rvp.install())).toBe("accepted");
  expect(await page.evaluate(() => window.__installPrompts)).toBe(1);
});

test("without the browser's offer the entry shows how to install from the browser's menu", async ({ page }) => {
  await page.goto("/");
  await ready(page);
  // Chromium decides after the load; with nothing sent the page falls back to the hint.
  const button = page.locator("#install");
  await expect(button).toBeVisible({ timeout: 10_000 });
  const state = await page.evaluate(() => window.rvp.installState());
  expect(state.state).toBe("hint");
  expect(state.hint).toMatch(/browser menu/i);
  await expect(page.locator("#install-hint")).toBeHidden();
  await button.click();
  const card = page.locator("#install-hint");
  await expect(card).toBeVisible();
  await expect(card).toHaveAttribute("role", "dialog");
  await expect(page.locator("#install-hint-text")).toHaveText(state.hint);
  await page.locator("#install-hint-close").click();
  await expect(card).toBeHidden();
  // The browser's offer arriving late turns the hint into the real button.
  await offerInstall(page);
  expect((await page.evaluate(() => window.rvp.installState())).state).toBe("available");
  await button.click();
  expect(await page.evaluate(() => window.__installPrompts)).toBe(1);
  // "Don't show again" is remembered.
  await page.reload();
  await ready(page);
  await expect(button).toBeVisible({ timeout: 10_000 });
  await button.click();
  await page.locator("#install-hint-never").click();
  await expect(button).toBeHidden();
  await expect(card).toBeHidden();
  await page.reload();
  await ready(page);
  await page.waitForFunction(() => window.rvp.installState().state === "hint", null, { timeout: 10_000 }); // the page has given up waiting for the offer
  await expect(button).toBeHidden();
});

test("the cross on the install button hides it for good", async ({ page }) => {
  await page.goto("/");
  await ready(page);
  await offerInstall(page);
  await expect(page.locator("#install")).toBeVisible();
  await page.locator("#install-dismiss").click();
  await expect(page.locator("#install")).toBeHidden();
  expect((await page.evaluate(() => window.rvp.installState())).dismissed).toBe(true);
  await page.reload();
  await ready(page);
  await offerInstall(page);
  await expect(page.locator("#install")).toBeHidden();
});

for (const [name, userAgent, extra, want] of [
  ["Safari on iPhone", "Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Mobile/15E148 Safari/604.1", { hasTouch: true, isMobile: true }, /Share, then Add to Home Screen/],
  ["Safari on a Mac", "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_4) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15", {}, /File > Add to Dock/],
  ["Firefox on a computer", "Mozilla/5.0 (X11; Linux x86_64; rv:125.0) Gecko/20100101 Firefox/125.0", {}, /Firefox does not install web apps on a computer/],
  ["Firefox on Android", "Mozilla/5.0 (Android 14; Mobile; rv:125.0) Gecko/125.0 Firefox/125.0", {}, /choose Install/],
]) {
  test(`the hint for ${name} says how to install there`, async ({ browser }) => {
    const context = await browser.newContext({ userAgent, ...extra });
    // These browsers have no `beforeinstallprompt`: the page shows the hint at once, without waiting for an offer.
    await context.addInitScript(() => { delete window.onbeforeinstallprompt; try { delete Window.prototype.onbeforeinstallprompt; } catch { /* kept */ } });
    const page = await context.newPage();
    await page.goto("/");
    await ready(page);
    await expect(page.locator("#install")).toBeVisible();
    const { state, hint } = await page.evaluate(() => window.rvp.installState());
    expect(state).toBe("hint");
    expect(hint).toMatch(want);
    await page.locator("#install").click();
    await expect(page.locator("#install-hint-text")).toHaveText(hint);
    await context.close();
  });
}

test("running as the installed app (standalone) there is no install entry", async ({ browser }) => {
  const context = await browser.newContext();
  await context.addInitScript(() => {
    const real = window.matchMedia.bind(window);
    window.matchMedia = (q) => {
      const m = real(q);
      return /display-mode:\s*standalone/.test(q) ? new Proxy(m, { get: (t, k) => (k === "matches" ? true : typeof t[k] === "function" ? t[k].bind(t) : t[k]) }) : m;
    };
  });
  const page = await context.newPage();
  await page.goto("/");
  await ready(page);
  await offerInstall(page);
  expect((await page.evaluate(() => window.rvp.installState())).state).toBe("installed");
  expect(await page.evaluate(() => window.rvp.install())).toBe("installed");
  expect(await page.evaluate(() => window.__installPrompts)).toBe(0);
  await page.waitForTimeout(3000); // longer than the page waits for the offer: it must stay away after that
  await expect(page.locator("#install")).toHaveCount(0);
  await expect(page.locator("#install-hint")).toHaveCount(0);
  await context.close();
});
