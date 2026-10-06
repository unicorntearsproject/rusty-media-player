// The installable-app side of the page: registers the service worker, offers "Install app" and "Update available", opens files the
// system hands to the installed app (file handlers, the share target). Everything here is optional: a browser without service
// workers, or a page opened from a file, simply runs as a normal page.

const dismissedKey = "rvp:install-dismissed";
/** Chromium decides after the page has loaded whether to offer installing; this long without the offer, the page shows the hint. */
const HINT_DELAY_MS = 2500;

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

/** What to tell someone whose browser has not offered installing: the way in for their browser. */
export function installHintFor(ua, touchPoints, secure) {
  if (!secure) return "Installing needs a secure page (https).";
  const ios = /iPhone|iPad|iPod/.test(ua) || (/Macintosh/.test(ua) && touchPoints > 1);
  if (ios) return "Tap Share, then Add to Home Screen.";
  if (/Firefox\//.test(ua) && !/Seamonkey/i.test(ua)) {
    return /Android/.test(ua)
      ? "Open the browser menu and choose Install."
      : "Firefox does not install web apps on a computer. Use Chrome, Edge or Brave and choose Install in the address bar or the menu, or get the Rusty Wave desktop app.";
  }
  if (/Chrome\/|Chromium\/|Edg\/|CriOS\//.test(ua)) {
    return "Open the browser menu and choose Install Rusty Wave (Install page as app, or Install this site as an app). If it is not listed, the app may already be installed.";
  }
  if (/Macintosh/.test(ua) && /Safari\//.test(ua)) return "In Safari, choose File > Add to Dock (or Share > Add to Home Screen).";
  return "Use your browser's menu: Install app, or Add to Home Screen.";
}

// ---- install state --------------------------------------------------------------------------------------
// `pending`: Chromium has not said yet; `available`: it has offered the install prompt; `hint`: it will not (Safari, Firefox, or
// nothing came in time), so the entry shows how to install from the browser's own menu; `installed`: running as the app.
let deferred = null;
let phase = "pending";
let hintOpen = false;
let installPill = null;
let installBar = null;
let hintCard = null;
let reportStatus = () => {};

// Kept from the first moment the module loads, because the browser fires the event once, early.
window.addEventListener("beforeinstallprompt", (e) => {
  e.preventDefault();
  deferred = e;
  phase = "available";
  render();
});

function dismissed() {
  return !!recall(dismissedKey);
}

/** `installed` | `available` | `hint` | `pending`, and the text of the hint. */
export function installState() {
  const installed = isInstalled();
  return {
    state: installed ? "installed" : phase,
    hint: installHintFor(navigator.userAgent, navigator.maxTouchPoints || 0, window.isSecureContext),
    dismissed: dismissed(),
  };
}

function render() {
  const { state } = installState();
  const show = (state === "available" || state === "hint") && !dismissed();
  if (!show) {
    if (installBar) installBar.hidden = true;
    closeHint();
    return;
  }
  if (!installBar) {
    // The button, and a small cross that hides it for good (the browser's own menu and `window.rvp.install()` stay).
    installBar = document.createElement("div");
    installBar.id = "install-bar";
    installBar.className = "pwa-bar";
    installPill = document.createElement("button");
    installPill.id = "install";
    installPill.type = "button";
    installPill.className = "pwa-pill";
    installPill.textContent = "Install app";
    installPill.setAttribute("aria-haspopup", "dialog");
    installPill.addEventListener("click", () => { install(); });
    const x = document.createElement("button");
    x.id = "install-dismiss";
    x.type = "button";
    x.className = "pwa-x";
    x.textContent = "\u00d7";
    x.setAttribute("aria-label", "Hide the install button");
    x.addEventListener("click", () => { remember(dismissedKey, "1"); render(); });
    installBar.append(installPill, x);
    document.body.appendChild(installBar);
  }
  installBar.hidden = false;
}

function closeHint() {
  hintOpen = false;
  if (hintCard) hintCard.hidden = true;
  if (installPill) installPill.setAttribute("aria-expanded", "false");
}

function openHint() {
  if (!hintCard) {
    hintCard = document.createElement("div");
    hintCard.id = "install-hint";
    hintCard.className = "pwa-card";
    hintCard.setAttribute("role", "dialog");
    hintCard.setAttribute("aria-label", "Install Rusty Wave");
    const title = document.createElement("strong");
    title.textContent = "Install Rusty Wave";
    const text = document.createElement("p");
    text.id = "install-hint-text";
    const row = document.createElement("div");
    row.className = "pwa-card-row";
    const close = document.createElement("button");
    close.type = "button";
    close.id = "install-hint-close";
    close.textContent = "Close";
    close.addEventListener("click", closeHint);
    const never = document.createElement("button");
    never.type = "button";
    never.id = "install-hint-never";
    never.textContent = "Don't show again";
    never.addEventListener("click", () => { remember(dismissedKey, "1"); render(); });
    row.append(close, never);
    hintCard.append(title, text, row);
    document.body.appendChild(hintCard);
  }
  hintCard.querySelector("#install-hint-text").textContent = installState().hint;
  hintCard.hidden = false;
  hintOpen = true;
  if (installPill) installPill.setAttribute("aria-expanded", "true");
  hintCard.querySelector("#install-hint-close").focus({ preventScroll: true });
}

/**
 * Install the app: the browser's own prompt where it offered one, else show how to install from the browser's menu. Resolves with
 * `accepted`, `dismissed`, `hint` (shown) or `installed` (already running as the app).
 */
export async function install() {
  if (isInstalled()) return "installed";
  if (!deferred) {
    if (hintOpen) closeHint(); else openHint();
    return "hint";
  }
  const offer = deferred;
  deferred = null; // the browser lets one event be used once
  if (installBar) installBar.hidden = true;
  offer.prompt();
  const choice = await offer.userChoice.catch(() => null);
  if (choice && choice.outcome === "accepted") return "accepted";
  remember(dismissedKey, "1");
  return "dismissed";
}

function watchInstall(status) {
  reportStatus = status;
  window.addEventListener("appinstalled", () => {
    deferred = null;
    phase = "installed";
    render();
    reportStatus("Rusty Wave was installed");
  });
  const mq = window.matchMedia("(display-mode: standalone)");
  if (mq.addEventListener) mq.addEventListener("change", render);
  // A browser without the event (Safari, Firefox) never offers; Chromium may take a moment.
  if (!("onbeforeinstallprompt" in window)) phase = "hint";
  else setTimeout(() => { if (phase === "pending") { phase = "hint"; render(); } }, HINT_DELAY_MS);
  render();
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

  watchInstall(api.status);

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
