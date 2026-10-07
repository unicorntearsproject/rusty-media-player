// AudioWorklet processor: pulls interleaved stereo float frames from a ring buffer that the page fills.
// It counts every frame it hands to the output and reports the count (with the audio clock time) so the
// player's master clock can follow what is actually playing.
class RvpRing extends AudioWorkletProcessor {
  constructor() {
    super();
    this.cap = Math.ceil(sampleRate * 3);
    this.bl = new Float32Array(this.cap); // planar ring: bulk copies instead of per-sample loops
    this.br = new Float32Array(this.cap);
    this.r = 0;
    this.size = 0;
    this.played = 0;
    this.paused = true;
    this.gen = 0;
    this.since = 0;
    // Underruns: frames of silence handed out while playing, after audio had arrived since the last flush (the end of a track counts too:
    // a test reads the counters mid-track). `runs` counts the stretches, `low` is the fewest frames the ring held at the start of a block.
    this.started = false;
    this.underFrames = 0;
    this.underRuns = 0;
    this.inUnder = false;
    this.low = Infinity;
    this.port.onmessage = (e) => {
      const m = e.data;
      if (m.t === "data") {
        if (m.gen !== this.gen) return; // from before a flush
        const l = new Float32Array(m.l);
        const r = new Float32Array(m.r);
        const n = Math.min(l.length, this.cap - this.size);
        const w = (this.r + this.size) % this.cap;
        const first = Math.min(n, this.cap - w);
        this.bl.set(l.subarray(0, first), w);
        this.br.set(r.subarray(0, first), w);
        if (n > first) {
          this.bl.set(l.subarray(first, n), 0);
          this.br.set(r.subarray(first, n), 0);
        }
        this.size += n;
        if (n > 0) this.started = true;
      } else if (m.t === "flush") {
        this.gen = m.gen;
        this.r = 0;
        this.size = 0;
        this.played = 0;
        this.started = false;
        this.underFrames = 0;
        this.underRuns = 0;
        this.inUnder = false;
        this.low = Infinity;
      } else if (m.t === "state") {
        this.paused = m.paused;
      } else if (m.t === "stats") {
        // A test (or the page) restarts the counters, for example once the start-up fill is over.
        this.underFrames = 0;
        this.underRuns = 0;
        this.low = Infinity;
      }
    };
  }

  process(_inputs, outputs) {
    const out = outputs[0];
    const L = out[0];
    const R = out[1] || out[0];
    const n = L.length;
    let i = 0;
    const had = this.size;
    if (!this.paused && this.started) this.low = Math.min(this.low, this.size);
    if (!this.paused && this.size > 0) {
      const take = Math.min(n, this.size);
      const first = Math.min(take, this.cap - this.r);
      L.set(this.bl.subarray(this.r, this.r + first), 0);
      R.set(this.br.subarray(this.r, this.r + first), 0);
      if (take > first) {
        L.set(this.bl.subarray(0, take - first), first);
        R.set(this.br.subarray(0, take - first), first);
      }
      this.r = (this.r + take) % this.cap;
      this.size -= take;
      this.played += take;
      i = take;
    }
    if (!this.paused && this.started && i < n) {
      this.underFrames += n - i;
      if (!this.inUnder) this.underRuns++;
      this.inUnder = true;
    } else if (had > 0) {
      this.inUnder = false;
    }
    for (; i < n; i++) { L[i] = 0; R[i] = 0; }
    this.since += n;
    if (this.since >= (this.paused ? 4096 : 1024)) {
      this.since = 0;
      this.port.postMessage({
        t: "status", played: this.played, time: currentTime + n / sampleRate, size: this.size,
        gen: this.gen, paused: this.paused, underFrames: this.underFrames, underRuns: this.underRuns,
        low: this.low === Infinity ? -1 : this.low,
      });
    }
    return true;
  }
}
registerProcessor("rvp-ring", RvpRing);
