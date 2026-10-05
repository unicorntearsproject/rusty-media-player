// A worker thread of the shared-memory build: sets up the wasm module on the shared memory once, then runs the
// closures the Rust side sends it (`rvp_worker_entry`) one after another.
import { Threads } from "./threads.js";

let glue = null;
let threads = null;
let queue = Promise.resolve();

self.onmessage = (e) => {
  // Messages are handled in order, and `init` is asynchronous, so chain them.
  // A trap in the wasm code (a panic) rejects here: tell the page, which restarts the player.
  queue = queue.then(() => handle(e.data)).catch((err) => {
    console.error("rvp worker crashed:", err);
    self.postMessage({ crashed: String((err && err.message) || err) });
  });
};

async function handle(d) {
  if (d.init) {
    glue = await import(d.glueUrl);
    await glue.default({ module_or_path: d.module, memory: d.memory, thread_stack_size: 4 << 20 });
    threads = new Threads(d.module, d.memory, d.glueUrl, false);
    return;
  }
  // Blocks this worker until the closure returns (a decoder thread runs until its decoder is dropped).
  glue.rvp_worker_entry(d.ptr, (ptr) => threads.spawn(ptr));
  self.postMessage("done");
}
