// The Media Session adapter: shows what is playing in the browser's media controls (lock screen, media keys,
// notification shade, picture-in-picture) and passes the buttons back to the Rust player as commands.
// The Rust side (`crates/rvp-host-web/src/media.rs`) calls setMetadata/setPlayback and polls takeCommand.

export class RvpMediaSession {
  constructor() {
    this.ms = typeof navigator !== "undefined" && "mediaSession" in navigator ? navigator.mediaSession : null;
    this.queue = [];
    this.artUrl = null;
    this.flags = { next: false, prev: false, seek: false };
    if (!this.ms) return;
    const on = (action, fn) => {
      try { this.ms.setActionHandler(action, fn); } catch { /* not supported in this browser */ }
    };
    on("play", () => this.queue.push("play"));
    on("pause", () => this.queue.push("pause"));
    on("stop", () => this.queue.push("stop"));
    on("seekto", (d) => { if (d && typeof d.seekTime === "number") this.queue.push(`seekto:${d.seekTime}`); });
    on("seekforward", (d) => this.queue.push(`seekby:${(d && d.seekOffset) || 10}`));
    on("seekbackward", (d) => this.queue.push(`seekby:${-((d && d.seekOffset) || 10)}`));
    on("nexttrack", () => this.queue.push("next"));
    on("previoustrack", () => this.queue.push("prev"));
  }

  /** What is playing. `art` is an encoded image (empty for none). */
  setMetadata(title, artist, album, art, mime) {
    if (!this.ms) return;
    if (this.artUrl) { URL.revokeObjectURL(this.artUrl); this.artUrl = null; }
    const artwork = [];
    if (art && art.length) {
      this.artUrl = URL.createObjectURL(new Blob([art.slice()], { type: mime || "image/jpeg" }));
      artwork.push({ src: this.artUrl, type: mime || "image/jpeg" });
    }
    try {
      this.ms.metadata = new MediaMetadata({ title, artist, album, artwork });
    } catch { /* MediaMetadata missing or the artwork was refused */ }
  }

  /** Transport state: 0 stopped, 1 playing, 2 paused. Times in seconds; `duration` < 0 when unknown. */
  setPlayback(state, position, duration, rate, canNext, canPrev, canSeek) {
    if (!this.ms) return;
    this.ms.playbackState = state === 1 ? "playing" : state === 2 ? "paused" : "none";
    try {
      if (duration > 0 && state !== 0) {
        this.ms.setPositionState({
          duration,
          position: Math.max(0, Math.min(position, duration)),
          playbackRate: rate > 0 ? rate : 1,
        });
      } else {
        this.ms.setPositionState();
      }
    } catch { /* position state was refused (inconsistent values) */ }
    // Only offer the buttons that do something.
    if (canNext !== this.flags.next || canPrev !== this.flags.prev || canSeek !== this.flags.seek) {
      this.flags = { next: canNext, prev: canPrev, seek: canSeek };
    }
  }

  /** The next command from the browser as text, or "". */
  takeCommand() {
    return this.queue.shift() || "";
  }
}
