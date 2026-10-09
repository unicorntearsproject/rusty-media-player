// Shared helpers of the Playwright specs. The rule for every wait in these tests: wait for the thing you need, never for an amount
// of time. State comes from `window.rvp.snapshot()`; "the page has drawn what I just did" is a number of animation frames
// (`frames`); "playback has moved on" is a change of position, of presented pictures or of the audio device's clock. A machine
// under load is slower, never different, and a wait that names its condition passes on both.

/**
 * The snapshot, after two animation frames: input is handled the moment it arrives, but the rectangles and the rows of the
 * snapshot are what the last frame drew, so what was just clicked or typed has been drawn when this returns.
 */
const snap = async (page) => {
  await frames(page, 2);
  return page.evaluate(() => window.rvp.snapshot());
};

/** Wait until `fn(arg)` is true in the page (polled every 50 ms by default; `"raf"` for every frame). */
const waitFor = (page, fn, arg, timeout, polling = 50) =>
  page.waitForFunction(fn, arg, { timeout: timeout || 20_000, polling });

const waitState = (page, state, timeout) => waitFor(page, (s) => window.rvp.snapshot().state === s, state, timeout);

/**
 * Wait for `n` animation frames of the page: what a key press, a click or a resize has changed has been drawn (and so has been laid
 * out, which is where the rectangles of the snapshot come from). Counts frames, not milliseconds.
 */
const frames = (page, n = 2) =>
  page.evaluate(
    (n) =>
      new Promise((resolve) => {
        const step = (k) => (k <= 0 ? resolve() : requestAnimationFrame(() => step(k - 1)));
        step(n);
      }),
    n,
  );

/** Press a key and let the page draw what it did (two frames), so the next key meets the layout it will act on. */
const press = async (page, key, n = 2) => {
  await page.keyboard.press(key);
  await frames(page, n);
};

/**
 * Wait until the page has run `n` more of its own frames (`window.rvp.perf().ticks`): a clock that counts what the page does and
 * does not care how long it takes. (A paused player still runs frames: the controls fade, the toasts age.)
 */
const ticks = async (page, n = 5) => {
  const t0 = await page.evaluate(() => window.rvp.perf().ticks);
  await waitFor(page, (t) => window.rvp.perf().ticks >= t, t0 + n, 30_000, "raf");
};

/**
 * Wait until `read` (run in the page) has returned the same value on `n` reads in a row, `gap` frames apart: it has settled.
 * Resolves with the value.
 */
const settled = async (page, read, { n = 3, gap = 2, timeout = 20_000 } = {}) => {
  const t0 = Date.now();
  let last = null;
  let same = 0;
  for (;;) {
    const v = JSON.stringify(await page.evaluate(read));
    same = v === last ? same + 1 : 0;
    last = v;
    if (same >= n) return JSON.parse(v);
    if (Date.now() - t0 > timeout) throw new Error(`did not settle: ${v}`);
    await frames(page, gap);
  }
};

/** Wait until the position has moved at least `us` microseconds on from where it is now; resolves with the new position. */
const playedFor = async (page, us) => {
  const p0 = (await snap(page)).position_us;
  await waitFor(page, (t) => window.rvp.snapshot().position_us >= t, p0 + us, 30_000);
  return (await snap(page)).position_us;
};

/**
 * How fast the position runs against the audio device's own clock (`AudioContext.currentTime`) over about `us` of playback: 1 is
 * real time at 1x, 2 is 2x. Both are read in the same moment. A window in which the player had to stop and refill its buffer (the
 * state left "playing") says nothing about the rate and is measured again, up to `tries` times: that is the one place these tests
 * retry, because a stall on a loaded machine is not what is being measured. `cap` bounds the wait for one window (wall time is only a cap).
 */
const rateAgainstDevice = async (page, us, tries = 4, cap = 30_000) => {
  const read = () =>
    page.evaluate(() => ({ p: window.rvp.snapshot().position_us, s: window.rvp.snapshot().state, t: window.rvp.audio().time }));
  let last;
  for (let i = 0; i < tries; i++) {
    const a = await read();
    await page.evaluate(() => {
      window.__stalled = false;
      window.__probe = setInterval(() => {
        if (window.rvp.snapshot().state !== "playing") window.__stalled = true;
      }, 20);
    });
    await waitFor(page, (t) => window.rvp.snapshot().position_us >= t, a.p + us, cap);
    const b = await read();
    const stalled = await page.evaluate(() => {
      clearInterval(window.__probe);
      return window.__stalled;
    });
    const device = (b.t - a.t) * 1e6;
    last = { moved: b.p - a.p, device, ratio: (b.p - a.p) / device, stalled: stalled || a.s !== "playing" };
    if (!last.stalled) return last;
  }
  return last;
};

/** Go to the `"library"` or `"player"` face (the first run opens on the Library; B switches, so press it only when needed). */
const toFace = async (page, face) => {
  if ((await snap(page)).lib.mode !== face) await press(page, "b");
  await waitFor(page, (f) => window.rvp.snapshot().lib.mode === f, face);
};

module.exports = { snap, waitFor, waitState, frames, press, ticks, settled, playedFor, rateAgainstDevice, toFace };
