// The page's audio engine, used by the Rust host (crates/rvp-host-web/src/audio.rs).
//
// Output goes through an AudioWorklet that reads a ring buffer (audio-worklet.js); browsers or contexts
// without worklets fall back to a ScriptProcessorNode with the same ring on the main thread. Either way the
// engine can say how many frames have been played since the last flush, which is what the player's audio
// master clock needs. The AudioContext is created up front (suspended until the first user gesture) and asked
// for 48 kHz so the player's pipeline rarely has to resample.
export class RvpAudio {
  constructor() {
    this.ctx = null;
    this.gain = null;
    this.node = null;
    this.mode = "none"; // "worklet" | "script" | "none"
    this.ready = false;
    this.pending = [];
    this.gen = 0;
    this.paused = true;
    this.volume = 1;
    this.rep = { played: 0, time: 0, size: 0, gen: 0, paused: true };
    this.sp = null; // script-processor ring
    this.#create();
  }

  #create() {
    const AC = window.AudioContext || window.webkitAudioContext;
    if (!AC) return;
    try {
      this.ctx = new AC({ latencyHint: "interactive", sampleRate: 48000 });
    } catch {
      this.ctx = new AC({ latencyHint: "interactive" });
    }
    this.gain = this.ctx.createGain();
    this.gain.gain.value = this.volume;
    this.gain.connect(this.ctx.destination);
    if (this.ctx.audioWorklet) {
      this.ctx.audioWorklet
        .addModule(new URL("audio-worklet.js", import.meta.url))
        .then(() => this.#startWorklet())
        .catch(() => this.#startScript());
    } else {
      this.#startScript();
    }
  }

  #startWorklet() {
    const node = new AudioWorkletNode(this.ctx, "rvp-ring", { numberOfInputs: 0, outputChannelCount: [2] });
    node.port.onmessage = (e) => {
      if (e.data.t === "status") this.rep = e.data;
    };
    node.connect(this.gain);
    this.node = node;
    this.mode = "worklet";
    this.ready = true;
    node.port.postMessage({ t: "flush", gen: this.gen });
    node.port.postMessage({ t: "state", paused: this.paused });
    for (const m of this.pending) if (m.gen === this.gen) node.port.postMessage(m, [m.l, m.r]);
    this.pending = [];
  }

  #startScript() {
    const size = 2048;
    const node = this.ctx.createScriptProcessor(size, 0, 2);
    const sp = { size, cap: Math.ceil(this.ctx.sampleRate * 3), r: 0, n: 0, played: 0 };
    sp.buf = new Float32Array(sp.cap * 2);
    node.onaudioprocess = (e) => {
      const L = e.outputBuffer.getChannelData(0);
      const R = e.outputBuffer.getChannelData(1);
      let i = 0;
      if (!this.paused) {
        for (; i < L.length && sp.n > 0; i++) {
          L[i] = sp.buf[sp.r * 2];
          R[i] = sp.buf[sp.r * 2 + 1];
          sp.r = (sp.r + 1) % sp.cap;
          sp.n--;
          sp.played++;
        }
      }
      for (; i < L.length; i++) { L[i] = 0; R[i] = 0; }
    };
    node.connect(this.gain);
    this.node = node;
    this.sp = sp;
    this.mode = "script";
    this.ready = true;
    for (const m of this.pending) if (m.gen === this.gen) this.#scriptWrite(m.inter);
    this.pending = [];
  }

  #scriptWrite(d) {
    const sp = this.sp;
    const n = d.length >> 1;
    let w = (sp.r + sp.n) % sp.cap;
    for (let i = 0; i < n && sp.n < sp.cap; i++) {
      sp.buf[w * 2] = d[i * 2];
      sp.buf[w * 2 + 1] = d[i * 2 + 1];
      w = (w + 1) % sp.cap;
      sp.n++;
    }
  }

  /** Call from a user gesture: browsers only let audio start after one. */
  unlock() {
    if (this.ctx && this.ctx.state === "suspended") this.ctx.resume().catch(() => {});
  }

  /** True while the browser is not letting audio run (no gesture yet). */
  get suspended() {
    return !this.ctx || this.ctx.state !== "running";
  }

  // ---- the interface the Rust sink calls ----

  sampleRate() {
    return this.ctx ? this.ctx.sampleRate : 0;
  }

  /** Frames played since the last flush, as of now. */
  played() {
    if (this.mode === "script") return this.sp.played;
    const r = this.rep;
    if (r.gen !== this.gen) return 0;
    if (this.paused || r.paused || !this.ctx) return r.played;
    const elapsed = Math.max(0, this.ctx.currentTime - r.time);
    return r.played + Math.min(elapsed * this.ctx.sampleRate, r.size);
  }

  /** Output latency in seconds. */
  latency() {
    if (!this.ctx) return 0;
    const base = (this.ctx.baseLatency || 0) + (this.ctx.outputLatency || 0);
    return this.mode === "script" ? base + this.sp.size / this.ctx.sampleRate : base;
  }

  write(samples) {
    // `samples` is a view into wasm memory that dies after this call: copy it out, splitting the channels.
    const n = samples.length >> 1;
    const l = new Float32Array(n);
    const r = new Float32Array(n);
    for (let i = 0; i < n; i++) { l[i] = samples[i * 2]; r[i] = samples[i * 2 + 1]; }
    if (this.mode === "script") {
      this.#scriptWrite(samples.slice());
    } else if (!this.ready) {
      this.pending.push({ t: "data", gen: this.gen, l: l.buffer, r: r.buffer, inter: samples.slice() });
    } else {
      this.node.port.postMessage({ t: "data", gen: this.gen, l: l.buffer, r: r.buffer }, [l.buffer, r.buffer]);
    }
  }

  flush() {
    this.gen++;
    this.rep = { played: 0, time: 0, size: 0, gen: this.gen, paused: true };
    this.pending = [];
    if (this.mode === "worklet") this.node.port.postMessage({ t: "flush", gen: this.gen });
    if (this.mode === "script") { this.sp.r = 0; this.sp.n = 0; this.sp.played = 0; }
  }

  setPaused(paused) {
    this.paused = paused;
    if (this.mode === "worklet") this.node.port.postMessage({ t: "state", paused });
  }

  setVolume(v) {
    this.volume = v;
    if (this.gain) this.gain.gain.setTargetAtTime(v, this.ctx.currentTime, 0.01);
  }

  debug() {
    return {
      mode: this.mode, state: this.ctx ? this.ctx.state : "none", sampleRate: this.sampleRate(),
      time: this.ctx ? this.ctx.currentTime : 0,
      played: this.played(), latency: this.latency(), paused: this.paused, volume: this.volume,
    };
  }
}
