// The library's share of the page: where its data is kept (IndexedDB), and how folders are picked and walked (the File System
// Access API where the browser has it, a `webkitdirectory` input where it does not). The Rust side asks for these things through
// effects (`folder`, `rescan:<id>`, `forget:<id>`) and gets a folder back as a listing: paths below the folder and `File` objects.

const DB_NAME = "rvp-library";
const KV = "kv";
const HANDLES = "handles";

/** What the library indexes (the Rust side filters again): audio files, folder pictures and playlist files. */
const WANTED = /\.(mp3|flac|ogg|oga|opus|wav|m4a|m4b|aac|mka|jpe?g|png|m3u8?|pls)$/i;

/** Big values (the library index and thumbnails) in IndexedDB, mirrored in a Map so a new player instance starts with them. */
export class RvpStore {
  constructor() {
    this.db = null;
    this.mem = new Map();
    this.handles = new Map();
    this.pending = Promise.resolve();
  }

  async open() {
    if (typeof indexedDB === "undefined") return this;
    try {
      this.db = await new Promise((resolve, reject) => {
        const req = indexedDB.open(DB_NAME, 1);
        req.onupgradeneeded = () => {
          req.result.createObjectStore(KV);
          req.result.createObjectStore(HANDLES);
        };
        req.onsuccess = () => resolve(req.result);
        req.onerror = () => reject(req.error);
      });
      await this.each(KV, (k, v) => this.mem.set(k, v instanceof Uint8Array ? v : new Uint8Array(v)));
      await this.each(HANDLES, (k, v) => this.handles.set(k, v));
    } catch (err) {
      console.warn("rvp: no IndexedDB, the library will not be kept between visits:", err);
      this.db = null;
    }
    return this;
  }

  each(store, fn) {
    return new Promise((resolve, reject) => {
      const cur = this.db.transaction(store).objectStore(store).openCursor();
      cur.onsuccess = () => {
        const c = cur.result;
        if (c) { fn(c.key, c.value); c.continue(); } else resolve();
      };
      cur.onerror = () => reject(cur.error);
    });
  }

  tx(store, fn) {
    if (!this.db) return;
    this.pending = this.pending.then(() => new Promise((resolve) => {
      try {
        const t = this.db.transaction(store, "readwrite");
        fn(t.objectStore(store));
        t.oncomplete = () => resolve();
        t.onerror = t.onabort = () => resolve();
      } catch { resolve(); }
    }));
  }

  /** Called by the Rust side: keep `bytes` under `key`; empty bytes delete. */
  put(key, bytes) {
    if (!bytes.length) {
      this.mem.delete(key);
      this.tx(KV, (s) => s.delete(key));
      return;
    }
    const copy = new Uint8Array(bytes); // the wasm memory may move or be reused
    this.mem.set(key, copy);
    this.tx(KV, (s) => s.put(copy, key));
  }

  putHandle(id, handle) {
    this.handles.set(id, handle);
    this.tx(HANDLES, (s) => s.put(handle, id));
  }

  deleteHandle(id) {
    this.handles.delete(id);
    this.tx(HANDLES, (s) => s.delete(id));
  }

  /** Resolves when everything written so far is stored (tests use it before reloading). */
  flush() { return this.pending; }
}

/** All the wanted files below a directory handle, as `{paths, files}` (paths relative to it, `/` separated). */
export async function walkHandle(dir) {
  const paths = [];
  const files = [];
  async function walk(d, prefix) {
    for await (const [name, h] of d.entries()) {
      if (h.kind === "directory") {
        await walk(h, `${prefix}${name}/`);
      } else if (WANTED.test(name)) {
        try {
          files.push(await h.getFile());
          paths.push(prefix + name);
        } catch { /* unreadable: skip it */ }
      }
    }
  }
  await walk(dir, "");
  return { paths, files };
}

/** The same for a `FileList` from a `webkitdirectory` input: the first component of each relative path is the folder's name. */
export function listFromInput(fileList) {
  const all = Array.from(fileList || []);
  if (!all.length) return null;
  const first = all[0].webkitRelativePath || "";
  const name = first.includes("/") ? first.split("/")[0] : "folder";
  const paths = [];
  const files = [];
  for (const f of all) {
    const rel = f.webkitRelativePath || f.name;
    const path = rel.includes("/") ? rel.slice(rel.indexOf("/") + 1) : rel;
    if (!WANTED.test(path)) continue;
    paths.push(path);
    files.push(f);
  }
  return { name, paths, files };
}

/** Walk a dropped directory entry (`webkitGetAsEntry`). */
export async function walkEntry(entry) {
  const paths = [];
  const files = [];
  const read = (reader) => new Promise((res, rej) => reader.readEntries(res, rej));
  const fileOf = (e) => new Promise((res, rej) => e.file(res, rej));
  async function walk(e, prefix) {
    if (e.isDirectory) {
      const reader = e.createReader();
      for (;;) {
        const batch = await read(reader);
        if (!batch.length) break;
        for (const child of batch) await walk(child, `${prefix}${e.name}/`);
      }
    } else if (WANTED.test(e.name)) {
      try {
        files.push(await fileOf(e));
        paths.push(prefix + e.name);
      } catch { /* skip */ }
    }
  }
  const reader = entry.createReader();
  for (;;) {
    const batch = await read(reader);
    if (!batch.length) break;
    for (const child of batch) await walk(child, "");
  }
  return { name: entry.name, paths, files };
}

/** True when the browser can show a directory picker that returns a handle we can keep. */
export const hasDirectoryPicker = () => typeof window.showDirectoryPicker === "function";

/** A root id for a folder name (the same folder picked again is the same root). */
export const rootId = (name) => `dir:${name}`;
