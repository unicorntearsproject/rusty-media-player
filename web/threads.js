// Worker threads for the shared-memory build (`pkg-mt`). The Rust side (crates/rvp-host-web/src/threads.rs) calls
// `spawn(ptr)` to start a thread: a Web Worker (`worker.js`) instantiates the same wasm module on the same shared
// memory and runs the boxed closure at `ptr`. Finished workers are reused, so starting a decoder after the first one
// is quick.
//
// Only the page's main thread creates Workers. A worker that wants another thread (a decoder thread starting the
// threads of its codec) asks the main thread by message: a worker busy inside wasm never returns to its event loop, and
// a Worker created from such a thread would never get to start.
export class Threads {
  /** @param {string} glueUrl absolute URL of the wasm-bindgen glue (pkg-mt/rvp.js) */
  constructor(module, memory, glueUrl, isMain = true) {
    this.module = module;
    this.memory = memory;
    this.glueUrl = glueUrl;
    this.isMain = isMain;
    this.idle = [];
    this.all = [];
    this.started = 0;
    this.failed = null;
  }

  /** Run the closure at `ptr` on a worker. */
  spawn(ptr) {
    if (!this.isMain) {
      self.postMessage({ spawn: ptr });
      return;
    }
    let w = this.idle.pop();
    if (!w) {
      w = new Worker(new URL("./worker.js", import.meta.url), { type: "module" });
      w.onmessage = (e) => {
        if (e.data === "done") this.idle.push(w);
        else if (e.data && e.data.spawn !== undefined) this.spawn(e.data.spawn);
        else if (e.data && e.data.crashed) this.failed = e.data.crashed;
      };
      w.onerror = (e) => {
        this.failed = e.message || String(e);
        console.error("rvp worker failed:", this.failed);
      };
      w.postMessage({ init: true, module: this.module, memory: this.memory, glueUrl: this.glueUrl });
      this.all.push(w);
      this.started++;
    }
    w.postMessage({ ptr });
  }

  /** Stop every worker (the instance they share is being thrown away). */
  terminate() {
    for (const w of this.all) w.terminate();
    this.all = [];
    this.idle = [];
  }
}
