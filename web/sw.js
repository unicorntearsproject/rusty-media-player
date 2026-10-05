// Service worker of Rusty Wave: keeps the app (the wasm, the scripts, the icons) in a versioned cache so it starts offline, and
// receives files shared to the installed app (the manifest's share target).
//
// `cargo xtask web` replaces the two placeholders below: VERSION is the Cargo version plus a hash of every file of the page, so any
// change to the page is a new service worker, which installs in the background and waits until the page asks to take over (the
// "Update available" button, see pwa.js). Old caches are deleted when the new worker activates.
const VERSION = "__RVP_VERSION__";
const PRECACHE = __RVP_PRECACHE__;
const CACHE = `rvp-app-${VERSION}`;
const SHARED = "rvp-shared";

self.addEventListener("install", (event) => {
  event.waitUntil((async () => {
    const cache = await caches.open(CACHE);
    // `reload`: do not let the browser's HTTP cache fill ours with something old.
    await cache.addAll(PRECACHE.map((u) => new Request(u, { cache: "reload" })));
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
      return await fetch(req);
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
