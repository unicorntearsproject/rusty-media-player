// Settings and the Theme dialog in the browser: Settings opens from the right-click menu and with Ctrl+, ; the Theme dialog takes pasted
// CSS (a `paste` event, as Ctrl+V sends it), previews it live on the canvas, applies it (kept across a reload) and resets it; a link is
// fetched by the page (the browser's own cross-origin rules apply: a site that allows it is previewed, one that does not tells the
// person to paste the CSS instead).
//
// The dialog is drawn on the canvas and has no rectangles in the snapshot, so it is driven from the keyboard the way a person would:
// the arrows walk the buttons and Enter presses one. The page's keyboard focus is mirrored here (it starts on the primary button
// and goes back to it when the set of buttons changes), see `Buttons`.
const { test, expect } = require("@playwright/test");
const http = require("node:http");
const { snap, waitFor, frames, press, toFace } = require("./helpers");

const center = (r) => [r.x + r.w / 2, r.y + r.h / 2];

const CSS = ":root{--background:#102030;--card:#1a2c40;--foreground:#f2f6ff;--primary:#00c2a8;--radius:4px}";

async function boot(page) {
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => m.type() === "error" && !/404|Failed to load resource/.test(m.text()) && errors.push(m.text()));
  await page.goto("/");
  await waitFor(page, () => window.rvp && window.rvp.ready);
  await toFace(page, "player"); // the first run opens on the Library; the right-click menu below is the player's
  return errors;
}

const dialog = async (page) => (await snap(page)).dialog;
const title = async (page) => (await dialog(page))?.title ?? null;
const bodyText = async (page) => ((await dialog(page))?.body ?? []).join(" | ");

/** The keyboard side of one dialog: which button the page's focus is on, worked out the way the page does. */
class Buttons {
  constructor(page) {
    this.page = page;
    this.shape = null;
    this.focus = 0;
  }

  /** Press the button called `label`. `reset`: the labels changed on the way (a fetch shows "Fetching…"), so the focus went back to the primary one. */
  async press(label, { reset = false } = {}) {
    const d = await dialog(this.page);
    const labels = d.buttons.map((b) => b.label);
    // The keyboard goes through the switches first, then the buttons.
    const nt = (d.toggles || []).length;
    const shape = labels.join("\u0001") + nt;
    if (reset || this.shape !== shape) {
      this.shape = shape;
      const primary = d.buttons.findIndex((b) => b.primary && b.enabled);
      this.focus = nt + (primary < 0 ? 0 : primary);
    }
    const want = nt + labels.indexOf(label);
    expect(want - nt, `a button "${label}" among ${labels}`).toBeGreaterThanOrEqual(0);
    expect(d.buttons[want - nt].enabled, `"${label}" is enabled`).toBe(true);
    while (this.focus !== want) {
      await press(this.page, "ArrowDown");
      this.focus = (this.focus + 1) % (nt + labels.length);
    }
    await press(this.page, "Enter");
  }
}

/** The mean colour of a part of the canvas the dialog's card does not cover (the top left corner). */
async function corner(page) {
  await frames(page, 3);
  const px = await page.evaluate(() => window.rvp.sample(0, 0, 160, 80, 4));
  const sum = [0, 0, 0];
  for (let i = 0; i < px.length; i += 3) for (let k = 0; k < 3; k++) sum[k] += px[i + k];
  const n = px.length / 3;
  return sum.map((v) => v / n);
}

const distance = (a, b) => Math.max(...a.map((v, i) => Math.abs(v - b[i])));

async function paste(page, text) {
  await page.evaluate((css) => {
    const dt = new DataTransfer();
    dt.setData("text/plain", css);
    window.dispatchEvent(new ClipboardEvent("paste", { clipboardData: dt }));
  }, text);
  await frames(page, 2);
}

async function openTheme(page) {
  await press(page, "Control+,");
  await waitFor(page, () => window.rvp.snapshot().dialog?.title === "Settings");
  const b = new Buttons(page);
  await b.press("Theme…");
  await waitFor(page, () => window.rvp.snapshot().dialog?.title === "Theme");
  return new Buttons(page);
}

/** Escape closes the dialog; from the Theme dialog it goes back to Settings first, so press it until nothing is left. */
async function closeDialog(page) {
  for (let i = 0; i < 3 && (await dialog(page)); i++) await press(page, "Escape");
  await waitFor(page, () => window.rvp.snapshot().dialog === null);
}

test.describe("Settings and the Theme dialog", () => {
  test("Settings opens from the right-click menu and with Ctrl+, and closes with Escape and its Close button", async ({ page }) => {
    const errors = await boot(page);
    expect(await dialog(page)).toBeNull();
    // The context menu has a Settings entry (its last row; the menu is taller than a 720 px window, so give it a taller one).
    await page.setViewportSize({ width: 1280, height: 800 });
    await frames(page, 3);
    await page.mouse.click(640, 300, { button: "right" });
    await waitFor(page, () => window.rvp.snapshot().menu_open);
    let s = await snap(page);
    const entry = s.menu.find((m) => m.label.startsWith("Settings"));
    expect(entry, "Settings in the menu").toBeTruthy();
    await page.mouse.click(...center(entry.rect));
    await waitFor(page, () => window.rvp.snapshot().dialog !== null);
    let d = await dialog(page);
    expect(d.title).toBe("Settings");
    expect(d.body.join(" ")).toMatch(/Rusty Wave \d+\.\d+\.\d+/);
    const labels = d.buttons.map((b) => b.label);
    expect(labels).toContain("Audio settings…");
    expect(labels).toContain("Theme…");
    expect(labels.at(-1)).toBe("Close");
    expect(d.buttons.at(-1).primary).toBe(true);
    await closeDialog(page);
    expect((await snap(page)).menu_open).toBe(false);
    // Ctrl+, opens it, and Enter on the primary button (Close) closes it.
    await press(page, "Control+,");
    await waitFor(page, () => window.rvp.snapshot().dialog?.title === "Settings");
    await press(page, "Enter");
    await waitFor(page, () => window.rvp.snapshot().dialog === null);
    // Audio settings, one level down, and back out with Escape.
    await press(page, "Control+,");
    await waitFor(page, () => window.rvp.snapshot().dialog?.title === "Settings");
    await new Buttons(page).press("Audio settings…");
    await waitFor(page, () => window.rvp.snapshot().dialog === null && window.rvp.snapshot().audio_panel !== null);
    await press(page, "Escape");
    await waitFor(page, () => window.rvp.snapshot().audio_panel === null);
    expect(errors).toEqual([]);
  });

  test("pasted CSS previews on the canvas, applies, survives a reload and resets", async ({ page }) => {
    const errors = await boot(page);
    const original = await corner(page);
    const b = await openTheme(page);
    expect(await bodyText(page)).toMatch(/Unicorn Tears/);
    const open = await corner(page); // the dialog's scrim over the original colours
    // Nothing to apply before there is something to preview.
    const d0 = await dialog(page);
    expect(d0.buttons.find((x) => x.label === "Apply").enabled).toBe(false);
    await paste(page, CSS);
    await b.press("Preview");
    await waitFor(page, () => window.rvp.snapshot().dialog?.body.join(" ").includes("Previewing"));
    expect(await bodyText(page)).toContain("Previewing Pasted theme");
    const previewed = await corner(page);
    expect(distance(previewed, open), "the canvas took the previewed colours").toBeGreaterThan(8);
    expect((await dialog(page)).buttons.find((x) => x.label === "Apply").enabled).toBe(true);
    // Closing without applying brings the old look back.
    await closeDialog(page);
    expect(distance(await corner(page), original), "the preview is gone").toBeLessThan(6);
    // Again, and apply this time.
    const b2 = await openTheme(page);
    await paste(page, CSS);
    await b2.press("Preview");
    await waitFor(page, () => window.rvp.snapshot().dialog?.body.join(" ").includes("Previewing"));
    await b2.press("Apply");
    await waitFor(page, () => (window.rvp.snapshot().toast || "").includes("Theme: Pasted theme"));
    expect(distance(await corner(page), previewed), "applied looks as it previewed (both under the dialog's scrim)").toBeLessThan(6);
    await closeDialog(page);
    const applied = await corner(page);
    expect(distance(applied, original)).toBeGreaterThan(8);
    // It is kept: after a reload the page starts in it.
    await page.evaluate(() => window.rvp.flushStore());
    await page.reload();
    await waitFor(page, () => window.rvp && window.rvp.ready);
    await toFace(page, "player");
    expect(distance(await corner(page), applied), "the theme came back").toBeLessThan(6);
    // Reset puts Unicorn Tears back, and that is kept too.
    const b3 = await openTheme(page);
    await b3.press("Reset to Unicorn Tears");
    await closeDialog(page);
    expect(distance(await corner(page), original), "back to the original").toBeLessThan(6);
    await page.evaluate(() => window.rvp.flushStore());
    await page.reload();
    await waitFor(page, () => window.rvp && window.rvp.ready);
    await toFace(page, "player");
    expect(distance(await corner(page), original), "the reset is kept").toBeLessThan(6);
    expect(errors).toEqual([]);
  });

  test("text that is not CSS says what is wrong and changes nothing", async ({ page }) => {
    const errors = await boot(page);
    const original = await corner(page);
    const b = await openTheme(page);
    await paste(page, "this is not a style sheet");
    await b.press("Preview");
    await frames(page, 4);
    expect(await bodyText(page)).not.toContain("Previewing");
    expect((await dialog(page)).buttons.find((x) => x.label === "Apply").enabled).toBe(false);
    await closeDialog(page);
    expect(distance(await corner(page), original)).toBeLessThan(6);
    expect(errors).toEqual([]);
  });

  test("a link to a style sheet that allows other sites to read it is previewed", async ({ page }) => {
    const errors = await boot(page);
    const original = await corner(page);
    let asked = 0;
    await page.route("https://design.example.test/**", (route) => {
      asked++;
      return route.fulfill({ status: 200, contentType: "text/css", headers: { "access-control-allow-origin": "*" }, body: CSS });
    });
    const b = await openTheme(page);
    const open = await corner(page);
    await paste(page, "https://design.example.test/tokens.css");
    await b.press("Preview");
    await waitFor(page, () => window.rvp.snapshot().dialog?.body.join(" ").includes("Previewing"));
    expect(asked).toBeGreaterThan(0);
    expect(await bodyText(page)).toContain("Previewing");
    expect(distance(await corner(page), open)).toBeGreaterThan(8);
    await closeDialog(page);
    expect(distance(await corner(page), original)).toBeLessThan(6);
    expect(errors).toEqual([]);
  });

  test("a link the browser may not read tells the person to paste the CSS instead", async ({ page }) => {
    const errors = await boot(page);
    const original = await corner(page);
    // A real server on another port serves the style sheet without the header that lets other sites read it: the browser enforces the
    // rule itself (Playwright's `route.fulfill` would add the header for us).
    const server = http.createServer((req, res) => {
      res.writeHead(200, { "content-type": "text/css" });
      res.end(CSS);
    });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    try {
      const b = await openTheme(page);
      const open = await corner(page);
      await paste(page, `http://127.0.0.1:${server.address().port}/tokens.css`);
      await b.press("Preview");
      await waitFor(page, () => /Couldn.t read that link/.test(window.rvp.snapshot().dialog?.body.join(" ") ?? ""));
      const text = await bodyText(page);
      expect(text).toMatch(/copy the style sheet's text and paste it here instead/);
      expect(text).not.toContain("Previewing");
      expect(distance(await corner(page), open), "nothing was previewed").toBeLessThan(6);
      expect((await dialog(page)).buttons.find((x) => x.label === "Apply").enabled).toBe(false);
      await closeDialog(page);
    } finally {
      await new Promise((resolve) => server.close(resolve));
    }
    // The browser says so in its console; that is the only complaint.
    expect(errors).toEqual([expect.stringContaining("blocked by CORS policy")]);
  });

  test("a server error is reported, and after a failure the dialog still previews another link", async ({ page }) => {
    const errors = await boot(page);
    const original = await corner(page);
    await page.route("https://broken.example.test/**", (route) =>
      route.fulfill({ status: 500, headers: { "access-control-allow-origin": "*" }, body: "no" }),
    );
    await page.route("https://open.example.test/**", (route) =>
      route.fulfill({ status: 200, contentType: "text/css", headers: { "access-control-allow-origin": "*" }, body: CSS }),
    );
    const b = await openTheme(page);
    const open = await corner(page);
    await paste(page, "https://broken.example.test/tokens.css");
    await b.press("Preview");
    await waitFor(page, () => /Couldn.t read that link \(the server said 500\)/.test(window.rvp.snapshot().dialog?.body.join(" ") ?? ""));
    expect(await bodyText(page)).not.toContain("Previewing");
    // A pasted link replaces the one in the box.
    await paste(page, "https://open.example.test/tokens.css");
    await b.press("Preview", { reset: true });
    await waitFor(page, () => window.rvp.snapshot().dialog?.body.join(" ").includes("Previewing"));
    expect(distance(await corner(page), open)).toBeGreaterThan(8);
    await closeDialog(page);
    expect(distance(await corner(page), original)).toBeLessThan(6);
    expect(errors).toEqual([]);
  });
});
