// Page glue for the Rust player: owns the DOM (canvas, file picker, drag and drop, fullscreen) and forwards
// input to the wasm `WebPlayer`. All drawing happens in Rust; this file only blits and forwards.
import { RvpAudio } from "./audio.js";
import { RvpMediaSession } from "./mediasession.js";
import { Threads } from "./threads.js";
import { setupPwa } from "./pwa.js";
import { RvpStore, hasDirectoryPicker, listFromInput, rootId, walkEntry, walkHandle } from "./library.js";

const canvas = document.getElementById("screen");
const fileInput = document.getElementById("file");
// A second, hidden picker for "add to playlist" so that it never replaces the playlist.
const addInput = document.getElementById("file-add");
// Folder picker fallback for the library, and the picker for playlist files.
const dirInput = document.getElementById("dir");
const playlistInput = document.getElementById("playlist-file");
const statusEl = document.getElementById("status");

// ---- the wasm module: shared-memory build with worker threads when the page can have it ------------------

// `?threads=0` forces the single-threaded build; the threaded build needs a cross-origin isolated page (the
// server sends COOP/COEP) and exists only if `cargo xtask web --threads` made it.
const wantThreads = new URLSearchParams(location.search).get("threads") !== "0" && !/(?:^|;\s*)rvp_threads=0/.test(document.cookie);
let glue = null;
let threads = null;
let threadInfo = { mode: "single", threads: 1, reason: "" };
let generation = 0;

/** Instantiate the wasm module (again, after a crash: each call gives a fresh instance and, with threads, fresh shared memory). */
async function loadWasm() {
  const gen = generation++;
  const bust = gen ? `?g=${gen}` : ""; // a new URL is a new glue module, so a new instance
  glue = null;
  threads = null;
  threadInfo = { mode: "single", threads: 1, reason: "" };
  if (wantThreads && self.crossOriginIsolated && typeof SharedArrayBuffer !== "undefined") {
    try {
      const url = new URL("./pkg-mt/rvp.js", import.meta.url);
      const wasmUrl = new URL("./pkg-mt/rvp_bg.wasm", import.meta.url);
      const head = await fetch(wasmUrl, { method: "HEAD" });
      if (head.ok) {
        glue = await import(url.href + bust);
        const module = await WebAssembly.compileStreaming(fetch(wasmUrl));
        const memory = new WebAssembly.Memory({ initial: 64, maximum: 32768, shared: true });
        await glue.default({ module_or_path: module, memory, thread_stack_size: 4 << 20 });
        const t = new Threads(module, memory, url.href + bust);
        threads = t;
        const n = glue.rvp_init_threads((ptr) => t.spawn(ptr), navigator.hardwareConcurrency || 4);
        threadInfo = { mode: "threads", threads: n, reason: "" };
      } else {
        threadInfo.reason = "no threaded build";
      }
    } catch (err) {
      console.warn("threaded build unavailable, using the single-threaded one:", err);
      glue = null;
      threads = null;
      threadInfo = { mode: "single", threads: 1, reason: String(err) };
    }
  } else if (wantThreads) {
    threadInfo.reason = "page is not cross-origin isolated";
  }
  if (!glue) {
    glue = await import("./pkg/rvp.js" + bust);
    await glue.default();
  }
}

await loadWasm();
// The library's data (index, covers) lives in IndexedDB; it is read once here so every player instance starts with it.
const store = await new RvpStore().open();
const audio = new RvpAudio();
const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
// Now-playing in the browser's media controls (lock screen, media keys) through the Media Session API.
const media = new RvpMediaSession();

// ---- crash recovery -------------------------------------------------------------------------------------
//
// A panic in Rust aborts the whole WebAssembly instance (a trap), and a decoder thread that traps leaves the shared
// state of the others in doubt. So the page does not try to resume inside the damaged instance: it throws it away,
// starts a fresh one and opens the same files again; the player's own resume position (saved every few seconds) puts
// playback back where it was.

let openedFiles = [];
let recoveries = 0;
let recoverStamps = [];
let recovering = null;
let wasmCrashed = false;

const isCrash = (e) => e instanceof WebAssembly.RuntimeError || /unreachable|memory access out of bounds/.test(String(e && e.message));

/** Wrap the player so a trap inside any call starts the recovery instead of breaking the page. */
function guarded(p) {
  return new Proxy(p, {
    get(target, name) {
      const v = target[name];
      if (typeof v !== "function") return v;
      return (...args) => {
        // The instance is being replaced: nothing may call into it.
        if (wasmCrashed) return name === "take_effects" ? [] : undefined;
        try {
          return v.apply(target, args);
        } catch (e) {
          if (isCrash(e)) {
            recover(e);
            return undefined;
          }
          throw e;
        }
      };
    },
  });
}

let player = null;
let lastSnapshot = { state: "idle" };
function makePlayer() {
  const p = new glue.WebPlayer(canvas, audio, reduceMotion.matches);
  p.set_media_session(media);
  p.set_store(store);
  for (const [key, bytes] of store.mem) p.store_preload(key, bytes);
  player = guarded(p);
  fit.done = false;
  fit();
  player.enable_visualizer(visualizerOn);
}
let visualizerOn = false;

async function recover(err) {
  if (recovering) return recovering;
  const now = performance.now();
  recoverStamps = recoverStamps.filter((t) => now - t < 60_000);
  if (recoverStamps.length >= 3) {
    statusEl.textContent = "The player stopped after repeated crashes. Reload the page.";
    console.error("rusty-wave: giving up after repeated crashes", err);
    return undefined;
  }
  recoverStamps.push(now);
  console.error("rusty-wave: the player crashed, restarting it:", err);
  wasmCrashed = true;
  recovering = (async () => {
    try {
      if (threads) threads.terminate();
      audio.flush();
      await loadWasm();
      wasmCrashed = false;
      makePlayer();
      relistFolders();
      recoveries++;
      if (openedFiles.length) player.open_files(openedFiles, false);
      player.toast("The player crashed and was restarted.");
      statusEl.textContent = "The player crashed and was restarted.";
    } catch (e) {
      console.error("rusty-wave: could not restart after the crash", e);
    } finally {
      recovering = null;
    }
  })();
  return recovering;
}

// ---- size -----------------------------------------------------------------------------------------------

function fit() {
  const dpr = window.devicePixelRatio || 1;
  const w = Math.max(1, Math.round(canvas.clientWidth * dpr));
  const h = Math.max(1, Math.round(canvas.clientHeight * dpr));
  if (w !== canvas.width || h !== canvas.height || !fit.done) {
    fit.done = true;
    player.resize(w, h, dpr);
  }
}
new ResizeObserver(fit).observe(canvas);
window.addEventListener("resize", fit);
makePlayer();
reduceMotion.addEventListener("change", () => player.set_reduce_motion(reduceMotion.matches));

// ---- effects the player asks for -----------------------------------------------------------------------

function effects() {
  for (const e of player.take_effects() || []) {
    if (e === "pick") {
      fileInput.value = "";
      fileInput.click();
    } else if (e === "add") {
      addInput.value = "";
      addInput.click();
    } else if (e === "folder") {
      addFolder();
    } else if (e.startsWith("rescan:")) {
      rescan(e.slice(7));
    } else if (e.startsWith("forget:")) {
      const id = e.slice(7);
      store.deleteHandle(id);
      listings.delete(id);
      connected.delete(id);
      player.library_connected([...connected]);
    } else if (e === "import") {
      playlistInput.value = "";
      playlistInput.click();
    } else if (e === "download") {
      for (const [name, mime, data] of player.take_downloads()) download(name, mime, data);
    } else if (e.startsWith("fullscreen:")) {
      setFullscreen(e.endsWith("true"));
    }
  }
}

// ---- library folders ------------------------------------------------------------------------------------

// What was last listed for each folder (so a restarted player instance can be given the same files again), and which folders
// are readable right now.
const listings = new Map();
const connected = new Set();

function giveListing(id, name, paths, files) {
  listings.set(id, { name, paths, files });
  connected.add(id);
  player.library_listing(id, name, paths, files);
  player.library_connected([...connected]);
  statusEl.textContent = `Read folder ${name}: ${paths.length} files`;
}

function relistFolders() {
  for (const [id, l] of listings) player.library_listing(id, l.name, l.paths, l.files);
  player.library_connected([...connected]);
}

async function scanHandle(handle, id = rootId(handle.name)) {
  statusEl.textContent = `Reading folder ${handle.name}…`;
  const { paths, files } = await walkHandle(handle);
  store.putHandle(id, handle);
  giveListing(id, handle.name, paths, files);
}

/** Pick a folder: the directory picker where there is one (it returns a handle we keep), else a folder input. Called inside the user's click. */
async function addFolder() {
  if (hasDirectoryPicker()) {
    let handle;
    try {
      handle = await window.showDirectoryPicker({ mode: "read", id: "rvp-music", startIn: "music" });
    } catch {
      return; // cancelled
    }
    await scanHandle(handle);
  } else {
    dirInput.value = "";
    dirInput.click();
  }
}

async function rescan(id) {
  const handle = store.handles.get(id);
  if (handle) {
    try {
      const mode = { mode: "read" };
      if ((await handle.queryPermission(mode)) === "granted" || (await handle.requestPermission(mode)) === "granted") {
        await scanHandle(handle, id);
        return;
      }
    } catch (err) {
      console.warn("rusty-wave: could not read the folder again:", err);
    }
  }
  addFolder(); // no handle, or no permission: ask for the folder again
}

dirInput.addEventListener("change", () => {
  const l = listFromInput(dirInput.files);
  if (l) giveListing(rootId(l.name), l.name, l.paths, l.files);
});
playlistInput.addEventListener("change", () => openFiles(playlistInput.files, true));

/** Folders the browser remembers: read the ones it lets us read without asking. */
async function restoreFolders() {
  for (const [id, handle] of store.handles) {
    try {
      if ((await handle.queryPermission({ mode: "read" })) === "granted") await scanHandle(handle, id);
    } catch { /* the folder is gone or the permission was withdrawn: it stays unconnected until the user rescans it */ }
  }
}

function download(name, mime, data) {
  const url = URL.createObjectURL(new Blob([data], { type: mime }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

function setFullscreen(on) {
  if (on && !document.fullscreenElement) {
    document.documentElement.requestFullscreen?.().catch(() => player.set_fullscreen_state(false));
  } else if (!on && document.fullscreenElement) {
    document.exitFullscreen?.().catch(() => {});
  }
}

document.addEventListener("fullscreenchange", () => {
  player.set_fullscreen_state(!!document.fullscreenElement);
  fit();
});

function openFiles(files, append = false) {
  files = Array.from(files || []);
  if (!files.length) return;
  audio.unlock();
  openedFiles = append ? openedFiles.concat(files) : files;
  player.open_files(files, append);
  const first = files.find((f) => !/\.(srt|vtt|ass|ssa)$/i.test(f.name)) || files[0];
  document.title = `${first.name} – Rusty Wave`;
  statusEl.textContent = files.length > 1 ? `Opened ${files.length} files` : `Opened ${first.name}`;
  if (audio.suspended) player.toast("Click anywhere to turn the sound on.");
}
const openFile = (file) => openFiles([file]);

fileInput.addEventListener("change", () => openFiles(fileInput.files));
addInput.addEventListener("change", () => openFiles(addInput.files, true));

// Keep the resume position safe when the page goes away.
window.addEventListener("pagehide", () => player.save_state());
document.addEventListener("visibilitychange", () => { if (document.hidden) player.save_state(); });

// ---- pointer --------------------------------------------------------------------------------------------

function pos(e) {
  const r = canvas.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  return [(e.clientX - r.left) * dpr, (e.clientY - r.top) * dpr];
}

canvas.addEventListener("pointermove", (e) => {
  const [x, y] = pos(e);
  player.pointer_move(x, y);
});
canvas.addEventListener("pointerdown", (e) => {
  audio.unlock();
  // The installable app: service worker, install and update buttons, files opened with the app or shared to it.
setupPwa({ openFiles, status: (t) => { statusEl.textContent = t; } });
// The manifest's "Music library" shortcut opens on the Library face.
if (new URLSearchParams(location.search).get("face") === "library") {
  player.key_down("b", false, false, false, false, false);
  player.key_up("b", false, false, false, false);
}
canvas.focus({ preventScroll: true });
restoreFolders();
  if (e.button === 0) canvas.setPointerCapture(e.pointerId);
  const [x, y] = pos(e);
  player.pointer_down(x, y, e.button);
  effects();
  if (e.button !== 0) e.preventDefault();
});
canvas.addEventListener("pointerup", (e) => {
  const [x, y] = pos(e);
  player.pointer_up(x, y, e.button);
  effects();
});
canvas.addEventListener("pointercancel", () => player.focus(false));
canvas.addEventListener("contextmenu", (e) => e.preventDefault());
canvas.addEventListener("auxclick", (e) => e.preventDefault());
canvas.addEventListener("wheel", (e) => {
  e.preventDefault();
  const k = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? 400 : 1;
  player.wheel(e.deltaX * k, e.deltaY * k);
}, { passive: false });

// ---- keyboard -------------------------------------------------------------------------------------------

window.addEventListener("keydown", (e) => {
  if (e.target instanceof HTMLInputElement) return;
  audio.unlock();
  const used = player.key_down(e.key, e.shiftKey, e.ctrlKey, e.altKey, e.metaKey, e.repeat);
  effects();
  if (used) e.preventDefault();
});
window.addEventListener("keyup", (e) => {
  if (e.target instanceof HTMLInputElement) return;
  player.key_up(e.key, e.shiftKey, e.ctrlKey, e.altKey, e.metaKey);
});
window.addEventListener("blur", () => player.focus(false));
window.addEventListener("focus", () => player.focus(true));

// ---- drag and drop --------------------------------------------------------------------------------------

let dragDepth = 0;
const hasFiles = (e) => e.dataTransfer && Array.from(e.dataTransfer.types || []).includes("Files");
window.addEventListener("dragenter", (e) => {
  if (!hasFiles(e)) return;
  e.preventDefault();
  dragDepth++;
  player.drag_over(true);
});
window.addEventListener("dragover", (e) => {
  if (!hasFiles(e)) return;
  e.preventDefault();
  e.dataTransfer.dropEffect = "copy";
});
window.addEventListener("dragleave", (e) => {
  if (!hasFiles(e)) return;
  dragDepth = Math.max(0, dragDepth - 1);
  if (dragDepth === 0) player.drag_over(false);
});
window.addEventListener("drop", (e) => {
  if (!hasFiles(e)) return;
  e.preventDefault();
  dragDepth = 0;
  player.drag_over(false);
  // A dropped folder joins the library; the files dropped with it play as usual.
  const entries = Array.from(e.dataTransfer.items || []).map((i) => (i.webkitGetAsEntry ? i.webkitGetAsEntry() : null)).filter(Boolean);
  const dirs = entries.filter((x) => x.isDirectory);
  if (dirs.length) {
    for (const d of dirs) walkEntry(d).then((l) => giveListing(rootId(l.name), l.name, l.paths, l.files));
    const loose = Array.from(e.dataTransfer.files).filter((f) => !dirs.some((d) => d.name === f.name && f.size === 0 && !f.type));
    if (loose.length) openFiles(loose, e.shiftKey);
    return;
  }
  // A drop plays what was dropped (several files make a playlist); Shift+drop adds to the current playlist.
  openFiles(e.dataTransfer.files, e.shiftKey);
});

// ---- frame loop -----------------------------------------------------------------------------------------

const perf = { ticks: 0, totalMs: 0, maxMs: 0, last: 0 };
function frame() {
  const t0 = performance.now();
  // A worker thread that died (a decoder that panicked) cannot be recovered from inside; start over.
  if (threads && threads.failed && !recovering) recover(threads.failed);
  if (!recovering) player.tick();
  const dt = performance.now() - t0;
  perf.ticks++;
  perf.totalMs += dt;
  perf.last = dt;
  if (dt > perf.maxMs) perf.maxMs = dt;
  const cursor = recovering ? "default" : player.cursor();
  if (canvas.style.cursor !== cursor) canvas.style.cursor = cursor;
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
// requestAnimationFrame stops in background tabs; keep audio and the clock fed.
setInterval(() => { if (document.hidden && !recovering) player.tick(); }, 25);

// ---- test and tooling hook ------------------------------------------------------------------------------

window.rvp = {
  ready: true,
  snapshot: () => {
    const j = player.snapshot();
    if (j) lastSnapshot = JSON.parse(j);
    return j ? lastSnapshot : { ...lastSnapshot, state: "recovering" };
  },
  openFile,
  openFiles,
  /** Give the library a folder picked with an input (tests use this and the fallback input). */
  addFolderFiles: (fileList) => {
    const l = listFromInput(fileList);
    if (l) giveListing(rootId(l.name), l.name, l.paths, l.files);
    return l ? l.paths.length : 0;
  },
  /** Everything written to IndexedDB so far is stored. */
  flushStore: () => store.flush(),
  saveState: () => player.save_state(),
  /** Switch the audio-analysis tap on or off, and read what it has seen (counts and the latest summary). */
  visualizer: (on) => { visualizerOn = on; player.enable_visualizer(on); },
  vizState: () => JSON.parse(player.viz_state()),
  audio: () => audio.debug(),
  /** RGBA bytes of a canvas region (physical pixels). */
  pixels: (x, y, w, h) => Array.from(canvas.getContext("2d").getImageData(x, y, w, h).data),
  canvasSize: () => [canvas.width, canvas.height],
  /** RGB samples of a canvas region every `step` pixels, as a flat array (cheap to ship to a test). */
  sample: (x, y, w, h, step = 4) => {
    const d = canvas.getContext("2d").getImageData(x, y, w, h).data;
    const out = [];
    for (let j = 0; j < h; j += step) {
      for (let i = 0; i < w; i += step) {
        const k = (j * w + i) * 4;
        out.push(d[k], d[k + 1], d[k + 2]);
      }
    }
    return out;
  },
  /** The canvas as a PNG data URL (to make or inspect goldens). */
  png: () => canvas.toDataURL("image/png"),
  /** Compare the canvas with a PNG data URL: mean absolute channel error and the share of pixels off by more than 8. */
  diffAgainst: async (dataUrl) => {
    const img = new Image();
    img.src = dataUrl;
    await img.decode();
    const c = document.createElement("canvas");
    c.width = img.width;
    c.height = img.height;
    const g = c.getContext("2d");
    g.drawImage(img, 0, 0);
    const want = g.getImageData(0, 0, c.width, c.height).data;
    const have = canvas.getContext("2d").getImageData(0, 0, canvas.width, canvas.height).data;
    if (c.width !== canvas.width || c.height !== canvas.height) return { sizeMismatch: [c.width, c.height, canvas.width, canvas.height] };
    let sum = 0, bad = 0;
    for (let i = 0; i < want.length; i += 4) {
      let m = 0;
      for (let k = 0; k < 3; k++) { const d = Math.abs(want[i + k] - have[i + k]); sum += d; if (d > m) m = d; }
      if (m > 8) bad++;
    }
    return { mean: sum / (want.length / 4 * 3), badFraction: bad / (want.length / 4) };
  },
  perf: () => ({ ...perf, avgMs: perf.totalMs / Math.max(1, perf.ticks) }),
  /** Worker threads in use: 1 for the single-threaded build. */
  threads: () => threadInfo.threads,
  /** How many times the page restarted the player after a crash. */
  recoveries: () => recoveries,
  /** Test hook: crash the player (`main`: panic now; `decoder`: the next video packet crashes its decoder). */
  debugCrash: (what) => (what === "decoder" ? player.debug_crash_decoder() : player.debug_panic()),
  threadInfo: () => threadInfo,
};
// The installable app: service worker, install and update buttons, files opened with the app or shared to it.
setupPwa({ openFiles, status: (t) => { statusEl.textContent = t; } });
// The manifest's "Music library" shortcut opens on the Library face.
if (new URLSearchParams(location.search).get("face") === "library") {
  player.key_down("b", false, false, false, false, false);
  player.key_up("b", false, false, false, false);
}
canvas.focus({ preventScroll: true });
