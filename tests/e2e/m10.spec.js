// M10 in the browser: the audio-first view. A folder of about 200 generated tracks is scanned through the page's folder input (and
// through the File System Access API against a folder in the origin-private file system), albums are opened and played with the
// right now-playing metadata, the visualizer's canvas moves with the music and stands still when paused, search, context menus,
// playlists (made, exported, imported), the library surviving a reload, and the switch between the Library and Player faces.
const { test, expect } = require("@playwright/test");
const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "../..");
const FIXTURES = process.env.RVP_FIXTURES || path.join(root, "target/fixtures");
const LIBRARY = path.join(FIXTURES, "library/music");
const SHOWCASE = path.join(FIXTURES, "showcase/music");
const VIDEO = path.join(FIXTURES, "av1_opus.webm");
const GOLDEN_DIR = path.join(__dirname, "golden");

const snap = (page) => page.evaluate(() => window.rvp.snapshot());
const waitFor = (page, fn, arg, timeout) => page.waitForFunction(fn, arg, { timeout: timeout || 20_000, polling: 30 });
const center = (r) => [r.x + r.w / 2, r.y + r.h / 2];

async function boot(page) {
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => m.type() === "error" && !/404/.test(m.text()) && errors.push(m.text()));
  await page.goto("/");
  await waitFor(page, () => window.rvp && window.rvp.ready);
  return errors;
}

/** Add a folder through the page's folder input and wait for the scan to finish. */
async function addFolder(page, dir, tracks) {
  await page.setInputFiles("#dir", dir);
  await waitFor(page, (n) => {
    const l = window.rvp.snapshot().lib;
    return l.tracks >= n && !l.scan;
  }, tracks, 60_000);
  await page.waitForTimeout(300);
}

async function library(page, dir = LIBRARY, tracks = 201) {
  const errors = await boot(page);
  await page.keyboard.press("b"); // to the Library face
  await waitFor(page, () => window.rvp.snapshot().lib.mode === "library");
  await addFolder(page, dir, tracks);
  return errors;
}

const ent = (s, label, kind) => s.lib.ents.find((e) => e.label === label && (!kind || e.kind === kind));

async function openAlbum(page, title) {
  let s = await snap(page);
  if (s.lib.view !== "albums" || s.lib.detail) {
    await page.keyboard.press("1");
    await waitFor(page, () => window.rvp.snapshot().lib.view === "albums" && !window.rvp.snapshot().lib.detail);
    await page.waitForTimeout(100);
  }
  // The grid scrolls: look for the card, scrolling down until it is on screen.
  for (let i = 0; i < 12; i++) {
    s = await snap(page);
    const e = ent(s, title, "album");
    if (e) {
      await page.mouse.click(...center(e.rect));
      await waitFor(page, () => window.rvp.snapshot().lib.detail !== null);
      await page.waitForTimeout(150);
      return;
    }
    await page.mouse.move(s.lib.body.x + 300, s.lib.body.y + 300);
    await page.mouse.wheel(0, 360);
    await page.waitForTimeout(80);
  }
  throw new Error(`album ${title} not found`);
}

test.describe("M10", () => {
  test("a) a folder is scanned into albums, artists and tracks; the views show them", async ({ page }) => {
    const errors = await library(page);
    let s = await snap(page);
    expect(s.lib.tracks).toBe(201);
    expect(s.lib.albums).toBe(20);
    expect(s.lib.artists).toBe(16);
    expect(s.lib.roots).toEqual([{ id: "dir:music", name: "music", connected: true }]);
    expect(s.lib.view).toBe("albums");
    // The album grid has cards with names; Unicode and compilations are intact.
    const titles = [];
    for (let i = 0; i < 14; i++) {
      s = await snap(page);
      for (const e of s.lib.ents) if (e.kind === "album" && !titles.includes(e.label)) titles.push(e.label);
      await page.mouse.move(s.lib.body.x + 300, s.lib.body.y + 300);
      await page.mouse.wheel(0, 300);
      await page.waitForTimeout(40);
    }
    for (const want of ["Polar Nights", "Corazón de Neón", "İstanbul Gecesi", "Neon Mixtape Vol. 1", "Road Trip Rips", "夜のドライブ", "Тишина", "Unknown Album"]) {
      expect(titles, want).toContain(want);
    }
    expect(titles.length).toBe(19); // two albums are called Unknown Album (by different artists)
    // Artists.
    await page.keyboard.press("2");
    await waitFor(page, () => window.rvp.snapshot().lib.view === "artists");
    await page.waitForTimeout(150);
    s = await snap(page);
    expect(s.lib.ents[0].label).toBe("A Tribe of Pines");
    expect(s.lib.ent_count).toBe(16);
    // Tracks: sorted by title, a click on a column head sorts by it, and again reverses it.
    await page.keyboard.press("3");
    await waitFor(page, () => window.rvp.snapshot().lib.view === "tracks");
    await page.waitForTimeout(150);
    s = await snap(page);
    expect(s.lib.ent_count).toBe(201);
    const first = s.lib.ents[0].label;
    await page.mouse.click(s.lib.sort.x + 20, s.lib.sort.y + 10); // the sort menu
    await waitFor(page, () => window.rvp.snapshot().lib.menu_open);
    await page.keyboard.press("Escape");
    expect(errors).toEqual([]);
    expect(first).toBeTruthy();
  });

  test("b) an album plays with the right now-playing metadata for each track, and the media session follows", async ({ page }) => {
    const errors = await library(page);
    await openAlbum(page, "Daybreak");
    expect((await snap(page)).lib.ent_count).toBe(10);
    await page.keyboard.press("Enter"); // nothing selected yet: no effect
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await waitFor(page, () => window.rvp.snapshot().state === "playing");
    // The queue is the album, in order.
    const album = (await snap(page)).playlist.map((p) => p.label);
    expect(album.length).toBe(10);
    // Poll the metadata as the ten short tracks go by (they are gapless): every track shows up, in order.
    const seen = [];
    const deadline = Date.now() + 15_000;
    while (Date.now() < deadline) {
      const s = await snap(page);
      const t = s.now.title;
      if (t && seen[seen.length - 1] !== t) {
        seen.push(t);
        expect(s.now.artist).toBe("Aurora Vale");
        expect(s.now.album).toBe("Daybreak");
        expect(s.now.has_art).toBe(true);
        const ms = await page.evaluate(() => (navigator.mediaSession && navigator.mediaSession.metadata ? { t: navigator.mediaSession.metadata.title, a: navigator.mediaSession.metadata.artist, al: navigator.mediaSession.metadata.album } : null));
        if (ms) expect(ms.al).toBe("Daybreak");
      }
      if (s.state === "ended" || seen.length === 10) break;
      await page.waitForTimeout(20);
    }
    // The queue is the whole album and starts at the row that was selected (the first one).
    expect(seen.length).toBeGreaterThanOrEqual(9);
    for (const t of seen) expect(album).toContain(t);
    const idx = seen.map((t) => album.indexOf(t));
    for (let i = 1; i < idx.length; i++) expect(idx[i]).toBe(idx[i - 1] + 1);
    expect(errors).toEqual([]);
  });

  test("c) the visualizer's pixels move with the music and stand still when it is paused", async ({ page }) => {
    const errors = await library(page, SHOWCASE, 40);
    await openAlbum(page, "Neon Rain");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await waitFor(page, () => window.rvp.snapshot().state === "playing");
    await page.keyboard.press("7");
    await waitFor(page, () => window.rvp.snapshot().lib.view === "visualizer");
    await waitFor(page, () => window.rvp.snapshot().viz.frames > 10);
    const region = async () => page.evaluate(() => window.rvp.sample(0, 100, 1280, 380, 8).join(","));
    // The first moments are silent (the audio has not reached the speakers yet): wait until the picture starts to move.
    let last = await region();
    for (let i = 0; i < 60; i++) {
      await page.waitForTimeout(100);
      const now = await region();
      if (now !== last) break;
      last = now;
    }
    const moving = [];
    for (let i = 0; i < 6; i++) {
      moving.push(await region());
      await page.waitForTimeout(160);
    }
    expect(new Set(moving).size).toBeGreaterThanOrEqual(5);
    // Pause, let the toast and the controls settle, then it must not change at all.
    await page.keyboard.press("Space");
    await waitFor(page, () => window.rvp.snapshot().state === "paused");
    await page.mouse.move(640, 300);
    await page.waitForTimeout(2800);
    const still = [];
    for (let i = 0; i < 6; i++) {
      still.push(await region());
      await page.waitForTimeout(200);
    }
    expect(new Set(still).size).toBe(1);
    // Playing again moves it. Effects step with the arrow keys and all of them draw.
    await page.keyboard.press("Space");
    await waitFor(page, () => window.rvp.snapshot().state === "playing");
    const names = new Set();
    for (let i = 0; i < 5; i++) {
      await page.keyboard.press("ArrowRight");
      await page.waitForTimeout(500);
      const s = await snap(page);
      names.add(s.viz.effect);
      const a = await region();
      await page.waitForTimeout(200);
      expect(await region(), s.viz.effect).not.toBe(a);
    }
    expect(names.size).toBe(5);
    // Escape leaves the visualizer for the now-playing screen.
    await page.keyboard.press("Escape");
    await waitFor(page, () => window.rvp.snapshot().lib.view === "nowplaying");
    expect(errors).toEqual([]);
  });

  test("d) search, keyboard navigation and context menus work with the keyboard and with the mouse", async ({ page }) => {
    const errors = await library(page);
    // Search: / focuses the box, typing filters (accents and case do not matter), Escape goes back.
    await page.keyboard.press("/");
    await waitFor(page, () => window.rvp.snapshot().lib.typing);
    await page.keyboard.type("zoe perez");
    await waitFor(page, () => window.rvp.snapshot().lib.query === "zoe perez");
    let s = await snap(page);
    expect(s.lib.view).toBe("search");
    expect(s.lib.ents.some((e) => e.kind === "artist" && e.label === "Zoë Pérez")).toBe(true);
    expect(s.lib.ents.some((e) => e.kind === "album" && e.label === "Corazón de Neón")).toBe(true);
    await page.keyboard.press("Escape");
    await waitFor(page, () => window.rvp.snapshot().lib.query === "" && window.rvp.snapshot().lib.view === "albums");
    // Keys that are shortcuts outside the box are text inside it, and shortcuts again after.
    await page.keyboard.press("m");
    await waitFor(page, () => window.rvp.snapshot().muted === true);
    await page.keyboard.press("m");
    // Tracks view: arrows select, the menu key opens the row's menu, "Add to queue" runs.
    await page.keyboard.press("3");
    await waitFor(page, () => window.rvp.snapshot().lib.view === "tracks");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await waitFor(page, () => window.rvp.snapshot().lib.selected === 1);
    await page.keyboard.press("ContextMenu");
    await waitFor(page, () => window.rvp.snapshot().menu_open);
    s = await snap(page);
    const labels = s.menu.map((m) => m.label);
    for (const want of ["Play", "Play next", "Add to queue", "Add to playlist", "Go to album", "Go to artist"]) expect(labels).toContain(want);
    const row = s.menu.find((m) => m.label === "Add to queue");
    await page.mouse.click(...center(row.rect));
    await waitFor(page, () => window.rvp.snapshot().playlist.length === 1);
    await waitFor(page, () => window.rvp.snapshot().state === "playing"); // an idle player starts what is queued
    // Right-click on a row opens the same menu at the pointer; Go to album opens the album.
    s = await snap(page);
    const target = s.lib.ents[3];
    await page.mouse.click(...center(target.rect), { button: "right" });
    await waitFor(page, () => window.rvp.snapshot().menu_open);
    s = await snap(page);
    await page.mouse.click(...center(s.menu.find((m) => m.label === "Go to album").rect));
    await waitFor(page, () => window.rvp.snapshot().lib.detail && window.rvp.snapshot().lib.detail.kind === "album");
    // Back with Backspace; the mouse's back button does the same one step further.
    await page.keyboard.press("Backspace");
    await waitFor(page, () => window.rvp.snapshot().lib.detail === null && window.rvp.snapshot().lib.view === "tracks");
    // The rail by mouse, the digits by keyboard.
    s = await snap(page);
    await page.mouse.click(...center(s.lib.rail.queue));
    await waitFor(page, () => window.rvp.snapshot().lib.view === "queue");
    await page.keyboard.press("6");
    await waitFor(page, () => window.rvp.snapshot().lib.view === "nowplaying");
    // Tab walks the zones; Enter on the bar's button pauses.
    await page.keyboard.press("Tab"); // rail
    await page.keyboard.press("Tab"); // bar
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("Enter");
    await waitFor(page, () => window.rvp.snapshot().state === "paused");
    expect(errors).toEqual([]);
  });

  test("e) playlists: made from an album, exported as M3U8 and PLS, imported again, kept after a reload", async ({ page }, testInfo) => {
    const errors = await library(page);
    await openAlbum(page, "Lofi Sketches");
    // The context menu of the first track: Add to playlist > New playlist...
    let s = await snap(page);
    await page.mouse.click(...center(s.lib.ents[0].rect), { button: "right" });
    await waitFor(page, () => window.rvp.snapshot().menu_open);
    s = await snap(page);
    await page.mouse.move(...center(s.menu.find((m) => m.label === "Add to playlist").rect));
    await page.waitForTimeout(150);
    s = await snap(page);
    await page.mouse.click(...center(s.menu.find((m) => m.label.startsWith("New playlist")).rect));
    await waitFor(page, () => window.rvp.snapshot().lib.typing);
    await page.keyboard.type("Chill mix");
    await page.keyboard.press("Enter");
    await waitFor(page, () => window.rvp.snapshot().lib.playlists === 1);
    s = await snap(page);
    expect(s.lib.playlist_list[0]).toMatchObject({ name: "Chill mix", tracks: 1, missing: 0 });
    // Add the whole album through the hero's menu path: the album card's context menu.
    await page.keyboard.press("Backspace");
    await page.waitForTimeout(150);
    s = await snap(page);
    await page.mouse.click(...center(ent(s, "Lofi Sketches", "album").rect), { button: "right" });
    await waitFor(page, () => window.rvp.snapshot().menu_open);
    s = await snap(page);
    await page.mouse.move(...center(s.menu.find((m) => m.label === "Add to playlist").rect));
    await page.waitForTimeout(150);
    s = await snap(page);
    await page.mouse.click(...center(s.menu.find((m) => m.label === "Chill mix").rect));
    await waitFor(page, () => window.rvp.snapshot().lib.playlist_list[0].tracks === 12);
    // Open it from the Playlists view and export both formats.
    await page.keyboard.press("4");
    await waitFor(page, () => window.rvp.snapshot().lib.view === "playlists");
    await page.waitForTimeout(150);
    s = await snap(page);
    await page.mouse.click(...center(s.lib.ents[0].rect));
    await waitFor(page, () => window.rvp.snapshot().lib.detail && window.rvp.snapshot().lib.detail.kind === "playlist");
    await page.waitForTimeout(150);
    s = await snap(page);
    const files = {};
    for (const [label, ext] of [["M3U8", "m3u8"], ["PLS", "pls"]]) {
      s = await snap(page);
      const button = s.lib.hero_buttons.find((h) => h.label === label);
      expect(button, label).toBeTruthy();
      const dl = page.waitForEvent("download", { timeout: 10_000 });
      await page.mouse.click(...center(button.rect));
      const d = await dl;
      expect(d.suggestedFilename().endsWith("." + ext)).toBe(true);
      files[d.suggestedFilename()] = fs.readFileSync(await d.path(), "utf8");
    }
    expect(Object.keys(files).sort()).toEqual(["Chill mix.m3u8", "Chill mix.pls"]);
    expect(files["Chill mix.m3u8"]).toMatch(/^#EXTM3U\n/);
    expect(files["Chill mix.m3u8"].split("\n").filter((l) => l.startsWith("#EXTINF")).length).toBe(12);
    expect(files["Chill mix.pls"]).toMatch(/^\[playlist\]\nFile1=Mono Mimi\/Lofi Sketches\//);
    // Import them again through the picker: two more playlists with the same tracks.
    const dir = testInfo.outputPath("playlists");
    fs.mkdirSync(dir, { recursive: true });
    const paths = [];
    for (const [name, text] of Object.entries(files)) {
      const p = path.join(dir, name);
      fs.writeFileSync(p, text);
      paths.push(p);
    }
    await page.setInputFiles("#playlist-file", paths);
    await waitFor(page, () => window.rvp.snapshot().lib.playlists === 3);
    s = await snap(page);
    expect(s.lib.playlist_list.map((p) => [p.tracks, p.missing])).toEqual([[12, 0], [12, 0], [12, 0]]);
    // A playlist with a file that is not in the library keeps it and says so.
    const odd = path.join(dir, "odd.m3u");
    fs.writeFileSync(odd, "﻿#EXTM3U\r\n#EXTINF:5,Gone\r\nNowhere/Gone.mp3\r\nMono Mimi/Lofi Sketches/01 Static Orbit.ogg\r\n");
    await page.setInputFiles("#playlist-file", odd);
    await waitFor(page, () => window.rvp.snapshot().lib.playlists === 4);
    s = await snap(page);
    expect(s.lib.playlist_list[3].tracks).toBe(2);
    expect(s.lib.playlist_list[3].missing).toBeGreaterThanOrEqual(1);
    // Everything survives a reload: the playlists and the library come back from IndexedDB without a scan.
    await page.evaluate(() => window.rvp.flushStore());
    await page.reload();
    await waitFor(page, () => window.rvp && window.rvp.ready);
    await waitFor(page, () => window.rvp.snapshot().lib.playlists === 4 && window.rvp.snapshot().lib.tracks === 201);
    s = await snap(page);
    expect(s.lib.roots[0].connected).toBe(false);
    expect(errors).toEqual([]);
  });

  test("f) the library survives a reload, covers included, and a second scan reads nothing", async ({ page }) => {
    const errors = await library(page);
    await page.evaluate(() => window.rvp.flushStore());
    await page.reload();
    await waitFor(page, () => window.rvp && window.rvp.ready);
    await page.keyboard.press("b");
    await waitFor(page, () => window.rvp.snapshot().lib.tracks === 201);
    await page.waitForTimeout(500);
    let s = await snap(page);
    expect(s.lib.albums).toBe(20);
    // The cards show their covers (flat colours, so a pixel in the middle of a card is the cover's colour, not the ink of a stand-in).
    const polar = ent(s, "Polar Nights", "album");
    const [cx, cy] = center(polar.rect);
    const [r, g, b] = await page.evaluate(([x, y]) => window.rvp.pixels(Math.round(x), Math.round(y - 20), 1, 1), [cx, cy]);
    expect(r).toBeGreaterThan(150);
    expect(g).toBeLessThan(110); // a red cover
    // The folder is not readable until it is picked again; doing so reads nothing new.
    expect(s.lib.roots[0].connected).toBe(false);
    await addFolder(page, LIBRARY, 201);
    s = await snap(page);
    expect(s.lib.roots[0].connected).toBe(true);
    expect(s.lib.tracks).toBe(201);
    expect(errors).toEqual([]);
  });

  test("g) the File System Access API: a folder is picked, walked and played", async ({ page }) => {
    const errors = await boot(page);
    // Put a few of the tracks in the origin-private file system and have the page's directory picker return it.
    const names = fs.readdirSync(path.join(LIBRARY, "Aurora Vale/Daybreak")).slice(0, 4);
    await page.evaluate(async (items) => {
      const rootDir = await navigator.storage.getDirectory();
      const music = await rootDir.getDirectoryHandle("opfs-music", { create: true });
      const alb = await (await music.getDirectoryHandle("Aurora Vale", { create: true })).getDirectoryHandle("Daybreak", { create: true });
      for (const [name, bytes] of items) {
        const fh = await alb.getFileHandle(name, { create: true });
        const w = await fh.createWritable();
        await w.write(new Uint8Array(bytes));
        await w.close();
      }
      window.showDirectoryPicker = async () => music;
    }, names.map((n) => [n, Array.from(fs.readFileSync(path.join(LIBRARY, "Aurora Vale/Daybreak", n)))]));
    await page.keyboard.press("b");
    await waitFor(page, () => window.rvp.snapshot().lib.mode === "library" && window.rvp.snapshot().lib.add_folder);
    // The add-folder button of the rail (a click is the user gesture the picker needs).
    let s = await snap(page);
    await page.mouse.click(...center(s.lib.add_folder));
    await waitFor(page, () => window.rvp.snapshot().lib.tracks === 4);
    s = await snap(page);
    expect(s.lib.albums).toBe(1);
    expect(s.lib.roots[0]).toMatchObject({ id: "dir:opfs-music", name: "opfs-music", connected: true });
    // The folder's files play: the scan gave them ids the player can open.
    await page.keyboard.press("1");
    await waitFor(page, () => window.rvp.snapshot().lib.ents.length === 1);
    s = await snap(page);
    await page.mouse.click(...center(s.lib.ents[0].rect));
    await waitFor(page, () => window.rvp.snapshot().lib.detail !== null);
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await waitFor(page, () => window.rvp.snapshot().state === "playing");
    expect((await snap(page)).now.album).toBe("Daybreak");
    expect(errors).toEqual([]);
  });

  test("h) audio opened from outside goes to the library face and video to the player; B switches without stopping anything", async ({ page }) => {
    const errors = await boot(page);
    await page.setInputFiles("#file", path.join(LIBRARY, "Aurora Vale/Daybreak", fs.readdirSync(path.join(LIBRARY, "Aurora Vale/Daybreak"))[0]));
    await waitFor(page, () => window.rvp.snapshot().state === "playing");
    await waitFor(page, () => window.rvp.snapshot().lib.mode === "library");
    let s = await snap(page);
    expect(s.lib.view).toBe("nowplaying");
    expect(s.now.artist).toBe("Aurora Vale");
    expect(s.now.has_art).toBe(true);
    await page.setInputFiles("#file", VIDEO);
    await waitFor(page, () => window.rvp.snapshot().lib.mode === "player");
    await waitFor(page, () => (window.rvp.snapshot().video || { presented: 0 }).presented > 5);
    // B: library face (the video keeps playing), B again: back to the picture.
    await page.keyboard.press("b");
    await waitFor(page, () => window.rvp.snapshot().lib.mode === "library");
    expect((await snap(page)).state).toBe("playing");
    await page.keyboard.press("b");
    await waitFor(page, () => window.rvp.snapshot().lib.mode === "player");
    // The same with the pointer: the button in the player's bar and the switch in the rail.
    s = await snap(page);
    await page.mouse.move(640, 300);
    await waitFor(page, () => window.rvp.snapshot().controls_opacity > 0.9);
    s = await snap(page);
    await page.mouse.click(...center(s.buttons.ModeSwitch));
    await waitFor(page, () => window.rvp.snapshot().lib.mode === "library");
    s = await snap(page);
    await page.mouse.click(...center(s.lib.mode_switch.player));
    await waitFor(page, () => window.rvp.snapshot().lib.mode === "player");
    expect(errors).toEqual([]);
  });

  test("i) the static views match their goldens", async ({ page }) => {
    await library(page);
    // The albums grid at the top, an album's page, and the track table.
    await page.mouse.move(1200, 700);
    const check = async (name) => {
      await page.waitForTimeout(500);
      const file = path.join(GOLDEN_DIR, name);
      const png = await page.evaluate(() => window.rvp.png());
      if (process.env.UPDATE_GOLDEN || !fs.existsSync(file)) {
        fs.mkdirSync(GOLDEN_DIR, { recursive: true });
        fs.writeFileSync(file, Buffer.from(png.split(",")[1], "base64"));
        console.log(`wrote ${file}`);
      }
      const golden = "data:image/png;base64," + fs.readFileSync(file).toString("base64");
      const d = await page.evaluate((g) => window.rvp.diffAgainst(g), golden);
      expect(d.sizeMismatch, `${name} size`).toBeUndefined();
      expect(d.mean, `${name} mean error`).toBeLessThan(1.5);
      expect(d.badFraction, `${name} bad pixels`).toBeLessThan(0.01);
    };
    await check("library-albums.png");
    await openAlbum(page, "Polar Nights");
    await page.mouse.move(1200, 700);
    await check("library-album.png");
    await page.keyboard.press("3");
    await waitFor(page, () => window.rvp.snapshot().lib.view === "tracks");
    await page.mouse.move(1200, 700);
    await check("library-tracks.png");
  });
});

test.describe("M10 with reduced motion", () => {
  test.use({ reducedMotion: "reduce" });

  test("j) the visualizer rests until it is asked for, and then runs calm", async ({ page }) => {
    const errors = await library(page, SHOWCASE, 40);
    await openAlbum(page, "Neon Rain");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await waitFor(page, () => window.rvp.snapshot().state === "playing");
    await page.keyboard.press("7");
    await waitFor(page, () => window.rvp.snapshot().lib.view === "visualizer");
    await page.waitForTimeout(1500);
    let s = await snap(page);
    expect(s.lib.viz_on).toBe(false);
    expect(s.viz.frames).toBe(0); // nothing is drawn, so nothing costs anything
    // The upper part of the screen (no text there) stays exactly the same while the music plays.
    const region = () => page.evaluate(() => window.rvp.sample(0, 80, 1280, 300, 8).join(","));
    const a = await region();
    await page.waitForTimeout(700);
    expect(await region()).toBe(a);
    // Enter turns it on; it moves, but gently: no frame differs from the one before it by much.
    await page.keyboard.press("Enter");
    await waitFor(page, () => window.rvp.snapshot().lib.viz_on === true);
    await waitFor(page, () => window.rvp.snapshot().viz.frames > 5);
    // The change since the previous call, measured inside the page: mean difference per colour channel.
    const change = () =>
      page.evaluate(() => {
        const c = document.getElementById("screen").getContext("2d").getImageData(0, 100, 1280, 300).data;
        const prev = window.__prev;
        window.__prev = c;
        if (!prev) return 0;
        let sum = 0;
        for (let k = 0; k < c.length; k += 4) sum += Math.abs(c[k] - prev[k]) + Math.abs(c[k + 1] - prev[k + 1]) + Math.abs(c[k + 2] - prev[k + 2]);
        return sum / (c.length / 4) / 3;
      });
    await change();
    let worst = 0;
    for (let i = 0; i < 12; i++) {
      await page.waitForTimeout(120);
      worst = Math.max(worst, await change());
    }
    // Mean change per pixel channel between samples 120 ms apart stays small (a flash would be tens of levels).
    expect(worst).toBeLessThan(12);
    expect(errors).toEqual([]);
  });
});

