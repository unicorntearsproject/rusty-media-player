// Platform video decoding through WebCodecs, for the codecs the player does not decode itself (HEVC; 10-bit H.264). The Rust side
// (`crates/rvp-host-web/src/webcodecs.rs`) asks `can()` synchronously, opens a decoder per stream and polls it each tick. Frames come
// back as RGBA: the browser's canvas does the colour work, tone-mapping HDR to SDR on the way.

/** The configs probed once at start; `can()` answers from what they said. */
const PROBES = {
  "hevc-main": "hvc1.1.6.L153.B0",
  "hevc-main10": "hvc1.2.4.L153.B0",
  "h264-high10": "avc1.6e0033",
};

export class RvpPlatformVideo {
  constructor() {
    this.supported = {};
    this.why = "";
    this.ready = this.probe();
  }

  /** WebCodecs exists in this browser (a secure page, a recent engine). */
  get available() {
    return typeof VideoDecoder === "function" && typeof EncodedVideoChunk === "function" && typeof OffscreenCanvas === "function";
  }

  async probe() {
    if (!this.available) {
      this.why = "this browser has no WebCodecs";
      return;
    }
    for (const [key, codec] of Object.entries(PROBES)) {
      try {
        const r = await VideoDecoder.isConfigSupported({ codec, hardwareAcceleration: "no-preference" });
        this.supported[key] = !!r.supported;
      } catch {
        this.supported[key] = false;
      }
    }
  }

  /** 1 supported, 0 not probed yet, -1 no. */
  can(key) {
    if (!this.available) return -1;
    if (!(key in this.supported)) return 0;
    return this.supported[key] ? 1 : -1;
  }

  reason() {
    return this.why;
  }

  open(codec, width, height, description) {
    return new RvpWebDecoder({ codec, codedWidth: width, codedHeight: height, description: description.slice(), optimizeForLatency: true });
  }
}

class RvpWebDecoder {
  constructor(config) {
    this.config = config;
    this.out = [];
    this.err = "";
    this.draining = false;
    this.canvas = null;
    this.ctx = null;
    this.start();
  }

  start() {
    this.dec = new VideoDecoder({
      output: (f) => this.onFrame(f),
      error: (e) => { this.err = (e && e.message) || String(e); },
    });
    this.dec.configure(this.config);
  }

  onFrame(f) {
    try {
      const w = f.displayWidth || f.codedWidth, h = f.displayHeight || f.codedHeight;
      if (!this.canvas || this.canvas.width !== w || this.canvas.height !== h) {
        this.canvas = new OffscreenCanvas(w, h);
        this.ctx = this.canvas.getContext("2d", { willReadFrequently: true, colorSpace: "srgb" });
      }
      this.ctx.drawImage(f, 0, 0, w, h);
      const img = this.ctx.getImageData(0, 0, w, h);
      this.out.push({ rgba: new Uint8Array(img.data.buffer), w, h, pts: f.timestamp });
    } catch (e) {
      this.err = (e && e.message) || String(e);
    } finally {
      f.close();
    }
  }

  /** The browser's error text since the last call, or "". */
  error() {
    const e = this.err;
    this.err = "";
    return e;
  }

  push(key, ptsUs, data) {
    if (this.dec.state === "closed") throw new Error(this.err || "the decoder closed");
    this.dec.decode(new EncodedVideoChunk({ type: key ? "key" : "delta", timestamp: ptsUs, data }));
  }

  /** Is there a decoded frame to take? */
  has() {
    return this.out.length > 0;
  }

  frameWidth() { return this.out[0].w; }
  frameHeight() { return this.out[0].h; }
  framePts() { return this.out[0].pts; }
  frameData() { return this.out[0].rgba; }
  pop() { this.out.shift(); }

  /** Packets waiting to be decoded, plus one while the end-of-stream flush is still running. */
  pending() {
    return this.dec.decodeQueueSize + (this.draining ? 1 : 0);
  }

  drain() {
    if (this.draining || this.dec.state !== "configured") return;
    this.draining = true;
    this.dec.flush().catch(() => {}).finally(() => { this.draining = false; });
  }

  /** After a seek: forget everything and start clean (the next packet is a key frame). */
  reset() {
    this.out = [];
    this.draining = false;
    if (this.dec.state === "closed") this.start();
    else { this.dec.reset(); this.dec.configure(this.config); }
  }

  close() {
    try { this.dec.close(); } catch { /* already closed */ }
    this.out = [];
  }
}
