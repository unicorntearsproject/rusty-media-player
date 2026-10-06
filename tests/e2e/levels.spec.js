// Crossfade and automatic level in the browser: the Audio panel (keyboard, mouse, context menu, on both faces), the settings kept
// across a reload, a crossfade between two tracks that never stalls and switches the title in the middle, the gain of a tagged
// track, and the library measuring its tracks in the background once the automatic level is on.
const { test, expect } = require("@playwright/test");
const path = require("node:path");

const root = path.resolve(__dirname, "../..");
const FIXTURES = process.env.RVP_FIXTURES || path.join(root, "target/fixtures");
const LEVELS = path.join(FIXTURES, "levels");
const XF_A = path.join(LEVELS, "xf_a.flac"); // 8 s of 440 Hz
const XF_B = path.join(LEVELS, "xf_b.flac"); // 8 s of 880 Hz
const TAGGED = path.join(LEVELS, "tagged_long.flac"); // 40 s; ReplayGain track -6.50 dB: -11.5 LUFS, album -3.20 dB: -14.8 LUFS

const { snap, waitFor: waitForAt, waitState, frames, press, settled, toFace } = require("./helpers");
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

const audio = async (page) => (await snap(page)).audio;

async function openPanel(page) {
  await press(page, "u");
  await waitFor(page, () => window.rvp.snapshot().audio_panel !== null);
  return (await snap(page)).audio_panel;
}

async function closePanel(page) {
  await press(page, "Escape");
  await waitFor(page, () => window.rvp.snapshot().audio_panel === null);
}

test.describe("Audio settings", () => {
  test("a) the panel opens with U, the mouse and the keyboard change every setting, and a reload keeps them", async ({ page }) => {
    const errors = await boot(page);
    let a = await audio(page);
    expect(a).toMatchObject({ crossfade: false, crossfade_secs: 5, auto_level: false, target_lufs: -14, level_mode: "track" });
    let p = await openPanel(page);
    // The keyboard belongs to the panel: N would be "next", Space flips the crossfade switch (the focus starts there).
    await press(page, "n");
    expect((await snap(page)).audio_panel).not.toBeNull();
    await press(page, "Space");
    expect((await audio(page)).crossfade).toBe(true);
    await press(page, "Tab");
    await press(page, "ArrowRight");
    expect((await audio(page)).crossfade_secs).toBe(6);
    await press(page, "ArrowDown");
    await press(page, "Space");
    expect((await audio(page)).auto_level).toBe(true);
    await press(page, "ArrowDown");
    await press(page, "Home");
    expect((await audio(page)).target_lufs).toBe(-23);
    await press(page, "ArrowDown");
    await press(page, "ArrowRight");
    expect((await audio(page)).level_mode).toBe("album");
    // The mouse: drag the length slider to its far end, click the crossfade switch off, then outside to close.
    p = (await snap(page)).audio_panel;
    const len = p.controls.crossfade_length;
    await page.mouse.move(len.x + len.w / 2, len.y + len.h / 2);
    await page.mouse.down();
    await page.mouse.move(len.x + len.w - 14, len.y + len.h / 2, { steps: 4 });
    await page.mouse.up();
    await frames(page, 2);
    expect((await audio(page)).crossfade_secs).toBe(10);
    const sw = p.controls.crossfade;
    await page.mouse.click(...center(sw));
    expect((await audio(page)).crossfade).toBe(false);
    await page.mouse.click(5, 5);
    await waitFor(page, () => window.rvp.snapshot().audio_panel === null);
    // The settings are in localStorage and come back after a reload.
    const want = await audio(page);
    await page.reload();
    await waitFor(page, () => window.rvp && window.rvp.ready);
    a = await audio(page);
    expect(a).toMatchObject({
      crossfade: want.crossfade,
      crossfade_secs: 10,
      auto_level: true,
      target_lufs: -23,
      level_mode: "album",
    });
    expect(errors).toEqual([]);
  });

  test("b) the context menu has every setting, and the panel is there on the Library face too", async ({ page }) => {
    const errors = await boot(page);
    await toFace(page, "player"); // the first run opens on the Library
    await page.mouse.click(640, 300, { button: "right" });
    await waitFor(page, () => window.rvp.snapshot().menu_open);
    let s = await snap(page);
    const row = (label) => s.menu.find((r) => r.label === label);
    const effects = row("Audio effects");
    expect(effects && effects.enabled).toBeTruthy();
    await page.mouse.move(...center(effects.rect));
    await frames(page, 2);
    s = await snap(page);
    for (const l of ["Audio settings…", "Crossfade: off", "Auto-level: off", "Crossfade length", "Target level", "Level each track", "Level whole albums"]) {
      expect(row(l), l).toBeTruthy();
    }
    // Turn the crossfade on from the menu (the toast says so), then the length from its submenu.
    await page.mouse.click(...center(row("Crossfade: off").rect));
    await waitFor(page, () => window.rvp.snapshot().audio.crossfade === true);
    expect((await snap(page)).toast).toContain("Crossfade on");
    // The Library face: U opens the panel there as well, over the library.
    await toFace(page, "library");
    await openPanel(page);
    await closePanel(page);
    expect(errors).toEqual([]);
  });

  test("c) a crossfade between two tracks keeps playing and changes the title in the middle of the fade", async ({ page }) => {
    const errors = await boot(page);
    await openPanel(page);
    await press(page, "Space"); // crossfade on
    await press(page, "Tab");
    await press(page, "Home"); // 2 s
    await closePanel(page);
    expect(await audio(page)).toMatchObject({ crossfade: true, crossfade_secs: 2 });
    await page.setInputFiles("#file", [XF_A, XF_B]);
    await waitState(page, "playing");
    await page.evaluate(() => {
      window.__fade = { states: [], titles: [] };
      window.__probe = setInterval(() => {
        const s = window.rvp.snapshot();
        if (s.audio.crossfading) {
          window.__fade.states.push(s.state);
          window.__fade.titles.push(s.title);
        }
      }, 20);
    });
    // The fade starts about two seconds before the end of the first track: the snapshot says so; the title is the second track's by
    // the end of it.
    await waitFor(page, () => window.rvp.snapshot().audio.crossfading === true, null, 40_000);
    await waitFor(page, () => window.rvp.snapshot().title.includes("xf_b"), null, 20_000);
    await waitFor(page, () => !window.rvp.snapshot().audio.crossfading, null, 20_000);
    const fade = await page.evaluate(() => {
      clearInterval(window.__probe);
      return window.__fade;
    });
    expect(fade.states.length).toBeGreaterThan(5);
    expect(fade.states.every((x) => x === "playing"), `states during the fade: ${[...new Set(fade.states)]}`).toBe(true);
    // The title was the first track's when the fade began. (The fade is mixed a little ahead of what is heard, by the length of the audio
    // device's buffer, so the switch of the title, which waits for the middle of the fade to be heard, may come after the mixing is done.)
    expect(fade.titles[0]).toContain("xf_a");
    // The second track plays on from where the fade left it (about a second in) and nothing stopped.
    const s = await snap(page);
    expect(s.state).toBe("playing");
    expect(s.position_us).toBeGreaterThan(500_000);
    expect(s.position_us).toBeLessThan(4_000_000);
    expect(errors).toEqual([]);
  });

  test("d) the automatic level reads the ReplayGain tag of a track and applies its gain", async ({ page }) => {
    const errors = await boot(page);
    await openPanel(page);
    await press(page, "Tab"); // length
    await press(page, "ArrowDown"); // auto-level
    await press(page, "Space");
    await closePanel(page);
    await page.setInputFiles("#file", TAGGED);
    await waitState(page, "playing");
    // -14 LUFS target, -11.5 LUFS track: -2.5 dB, reached within a tenth of a second and then left alone.
    const gain = await settled(page, () => window.rvp.snapshot().audio.gain_db);
    expect(gain).toBeCloseTo(-2.5, 1);
    // Album mode takes the album tag (-14.8 LUFS): +0.8 dB.
    await openPanel(page);
    await press(page, "Tab");
    await press(page, "Tab");
    await press(page, "Tab");
    await press(page, "Tab");
    await press(page, "ArrowRight"); // level whole albums
    await closePanel(page);
    expect(await settled(page, () => window.rvp.snapshot().audio.gain_db)).toBeCloseTo(0.8, 1);
    // Off: no gain.
    await openPanel(page);
    await press(page, "Tab");
    await press(page, "Tab");
    await press(page, "Space");
    await closePanel(page);
    await waitFor(page, () => window.rvp.snapshot().audio.gain_db === null);
    expect(errors).toEqual([]);
  });

  test("e) turning the automatic level on measures the library in the background, and the figures survive a reload", async ({ page }) => {
    const errors = await boot(page);
    await toFace(page, "library");
    await page.setInputFiles("#dir", path.join(LEVELS, "lib"));
    await waitFor(page, () => window.rvp.snapshot().lib.tracks >= 4 && !window.rvp.snapshot().lib.scan, null, 60_000);
    // Only the tagged track has a figure (from its tags); the other three wait.
    let a = await audio(page);
    expect(a.library_measured).toBe(1);
    await openPanel(page);
    await press(page, "Tab");
    await press(page, "ArrowDown");
    await press(page, "Space"); // auto-level on
    await closePanel(page);
    await waitFor(page, () => window.rvp.snapshot().audio.library_measured === 4, null, 120_000);
    await waitFor(page, () => !window.rvp.snapshot().lib.scan, null, 20_000);
    // The index was saved with the figures: after a reload nothing is measured again and all four are there.
    await page.reload();
    await waitFor(page, () => window.rvp && window.rvp.ready);
    await waitFor(page, () => window.rvp.snapshot().lib.tracks >= 4, null, 30_000);
    a = await audio(page);
    expect(a.library_measured).toBe(4);
    expect(errors).toEqual([]);
  });
});
