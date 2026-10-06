// Service worker of Rusty Wave: keeps the app (the wasm, the scripts, the icons) in a versioned cache so it starts offline, and
// receives files shared to the installed app (the manifest's share target).
//
// `cargo xtask web` replaces the two placeholders below: VERSION is the Cargo version plus a hash of every file of the page, so any
// change to the page is a new service worker, which installs in the background and waits until the page asks to take over (the
// "Update available" button, see pwa.js). Old caches are deleted when the new worker activates.
const VERSION = "__RVP_VERSION__";
// The page's own files, and the two wasm builds: the worker keeps only the one this browser can run (the threaded build needs a
// cross-origin isolated page and shared memory, the same test the page makes), so nothing downloads that is never used.
const PRECACHE = __RVP_PRECACHE__;
const PKG = __RVP_PKG__;
const PKG_MT = __RVP_PKG_MT__;
const OTHER_BUILD = new Set(PKG.concat(PKG_MT).map((u) => new URL(u, self.location).href));
const CACHE = `rvp-app-${VERSION}`;
const SHARED = "rvp-shared";

/**
 * Whether the pages of this site are cross-origin isolated (the threaded wasm build needs that). A service worker's own
 * `crossOriginIsolated` is not the pages' (it is false in some browsers even where the pages are isolated), so ask the server: the page
 * is isolated when it is served with COOP `same-origin` and COEP `require-corp` or `credentialless`.
 */
async function pagesAreIsolated() {
  try {
    const r = await fetch("./", { cache: "no-cache" });
    const coop = (r.headers.get("cross-origin-opener-policy") || "").toLowerCase();
    const coep = (r.headers.get("cross-origin-embedder-policy") || "").toLowerCase();
    return coop === "same-origin" && (coep === "require-corp" || coep === "credentialless");
  } catch (err) {
    return self.crossOriginIsolated === true;
  }
}

self.addEventListener("install", (event) => {
  event.waitUntil((async () => {
    const cache = await caches.open(CACHE);
    const threaded = PKG_MT.length > 0 && (await pagesAreIsolated());
    const urls = PRECACHE.concat(threaded ? PKG_MT : PKG);
    // `no-cache`: revalidate with the server, so a file the browser has just fetched answers with a 304 and nothing downloads twice, and
    // a stale HTTP-cache entry never fills ours (the names carry a hash, so a changed file is a new URL anyway).
    await cache.addAll(urls.map((u) => new Request(u, { cache: "no-cache" })));
  })());
});

self.addEventListener("activate", (event) => {
  event.waitUntil((async () => {
    for (const key of await caches.keys()) {
      if (key.startsWith("rvp-app-") && key !== CACHE) await caches.delete(key);
    }
    await self.clients.claim();
  })());
});

self.addEventListener("message", (event) => {
  const type = event.data && event.data.type;
  if (type === "SKIP_WAITING") self.skipWaiting();
  if (type === "VERSION" && event.source) event.source.postMessage({ type: "VERSION", version: VERSION });
});

self.addEventListener("fetch", (event) => {
  const req = event.request;
  const url = new URL(req.url);
  if (url.origin !== self.location.origin) return;
  if (req.method === "POST" && url.pathname.endsWith("/share-target")) {
    event.respondWith(receiveShare(req));
    return;
  }
  if (req.method !== "GET") return;
  event.respondWith((async () => {
    const cache = await caches.open(CACHE);
    // The page asks for `pkg/rvp.js?g=1` after a crash to get a fresh module instance: same file, so ignore the query.
    const hit = await cache.match(req, { ignoreSearch: true });
    if (hit) return hit;
    try {
      const res = await fetch(req);
      // The wasm build that was not precached (the other one of the two) is kept the first time the page asks for it, so a browser that
      // falls back to it (the threads did not start) still starts offline next time.
      if (res.ok && OTHER_BUILD.has(url.href)) cache.put(req, res.clone()).catch(() => {});
      return res;
    } catch (err) {
      if (req.mode === "navigate") {
        const shell = await cache.match("./", { ignoreSearch: true });
        if (shell) return shell;
      }
      throw err;
    }
  })());
});

/** Files shared to the app (POST from the system share sheet): keep them for the page, which opens them after the redirect. */
async function receiveShare(req) {
  try {
    const form = await req.formData();
    const cache = await caches.open(SHARED);
    let i = 0;
    for (const f of form.getAll("files")) {
      if (typeof f === "string") continue;
      const key = `./shared/${Date.now()}-${i++}`;
      await cache.put(key, new Response(f, { headers: { "Content-Type": f.type || "application/octet-stream", "X-Rvp-Name": encodeURIComponent(f.name) } }));
    }
  } catch (err) {
    // Nothing to keep; the page opens empty.
  }
  return Response.redirect("./?share-target", 303);
}
