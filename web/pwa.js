// The installable-app side of the page: registers the service worker, offers "Install app" and "Update available", opens files the
// system hands to the installed app (file handlers, the share target). Everything here is optional: a browser without service
// workers, or a page opened from a file, simply runs as a normal page.

const dismissedKey = "rvp:install-dismissed";

function remember(key, value) {
  try { localStorage.setItem(key, value); } catch { /* storage may be blocked */ }
}

function recall(key) {
  try { return localStorage.getItem(key); } catch { return null; }
}

/** A small themed button in the corner; `kind` is only for tests. */
function pill(id, text, onClick) {
  const b = document.createElement("button");
  b.id = id;
  b.type = "button";
  b.className = "pwa-pill";
  b.textContent = text;
  b.addEventListener("click", onClick);
  document.body.appendChild(b);
  return b;
}

export function isInstalled() {
  return window.matchMedia("(display-mode: standalone)").matches || window.navigator.standalone === true;
}

/**
 * @param {{ openFiles: (files: File[], append?: boolean) => void, status: (text: string) => void }} api
 */
export function setupPwa(api) {
  // Files the installed app was opened with ("Open with Rusty Wave" on a media file).
  if ("launchQueue" in window) {
    window.launchQueue.setConsumer(async (params) => {
      if (!params.files || !params.files.length) return;
      const files = [];
      for (const h of params.files) {
        try { files.push(await h.getFile()); } catch { /* the file went away */ }
      }
      if (files.length) api.openFiles(files);
    });
  }

  // Files shared to the app: the service worker kept them, we open them and let go of them.
  if (location.search.includes("share-target") && "caches" in window) {
    (async () => {
      const cache = await caches.open("rvp-shared");
      const files = [];
      for (const req of await cache.keys()) {
        const res = await cache.match(req);
        const name = decodeURIComponent(res.headers.get("X-Rvp-Name") || "shared");
        files.push(new File([await res.blob()], name, { type: res.headers.get("Content-Type") || "" }));
        await cache.delete(req);
      }
      history.replaceState(null, "", location.pathname);
      if (files.length) api.openFiles(files);
    })().catch(() => {});
  }

  // The install button, where the browser offers installing.
  let deferred = null;
  let installButton = null;
  window.addEventListener("beforeinstallprompt", (e) => {
    e.preventDefault();
    deferred = e;
    if (recall(dismissedKey) || isInstalled()) return;
    installButton = pill("install", "Install app", async () => {
      installButton.hidden = true;
      deferred.prompt();
      const choice = await deferred.userChoice.catch(() => null);
      if (!choice || choice.outcome !== "accepted") remember(dismissedKey, "1");
      deferred = null;
    });
  });
  window.addEventListener("appinstalled", () => {
    if (installButton) installButton.hidden = true;
    api.status("Rusty Wave was installed");
  });

  // The service worker, and its update flow.
  if (!("serviceWorker" in navigator) || !(window.isSecureContext)) return;
  // The new worker takes over when the user asks; only then does the page reload (the first install also fires `controllerchange`,
  // when the worker claims the page, and that must not reload anything).
  let updating = false;
  navigator.serviceWorker.addEventListener("controllerchange", () => {
    if (!updating) return;
    updating = false;
    location.reload();
  });
  navigator.serviceWorker
    .register("sw.js", { scope: "./" })
    .then((reg) => {
      const offer = (worker) => {
        if (document.getElementById("update")) return;
        const b = pill("update", "Update available: reload", () => {
          updating = true;
          worker.postMessage({ type: "SKIP_WAITING" });
        });
        b.classList.add("pwa-pill-update");
        api.status("A new version of Rusty Wave is ready");
      };
      if (reg.waiting && navigator.serviceWorker.controller) offer(reg.waiting);
      reg.addEventListener("updatefound", () => {
        const w = reg.installing;
        if (!w) return;
        w.addEventListener("statechange", () => {
          if (w.state === "installed" && navigator.serviceWorker.controller) offer(w);
        });
      });
      // Look for a new version when the app comes back to the foreground.
      document.addEventListener("visibilitychange", () => { if (!document.hidden) reg.update().catch(() => {}); });
    })
    .catch((err) => console.warn("rusty-wave: no service worker:", err));
}
