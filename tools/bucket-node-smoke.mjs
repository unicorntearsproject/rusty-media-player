// A stand-in host for the Rusty Bucket module, in Node (V8): runs the wasm module's `bucket_main` lifecycle against a tiny
// scripted `bucket_v0` (until the Bucket Simulator exists) and, for a threads build with the `smoke` export, checks the thread
// start-up contract (each thread on its own stack and TLS block) with real Workers on shared memory.
//
//   node tools/bucket-node-smoke.mjs <module.wasm> [--threads] [--smoke-threads N] [--seconds S]
//
// Prints one line per check and exits non-zero if one fails. Not a simulator: it implements only what the checks need and answers
// -UNSUPPORTED to the rest, as a host that predates a function does.
import { Worker, isMainThread, parentPort, workerData } from 'node:worker_threads';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const ERR = { INVALID: -1, NOT_FOUND: -2, UNSUPPORTED: -6 };
const CAPS = { THREADS: 1n << 0n, ATOMICS: 1n << 2n, CANVAS: 1n << 3n, AUDIO_OUT: 1n << 5n, NOW_PLAYING: 1n << 7n, LIBRARY: 1n << 9n };
const EV = { RESIZE: 7, TERMINATE: 20 };
const T0 = performance.now();
const nowUs = () => BigInt(Math.round((performance.now() - T0) * 1000)) + 1_000_000n;

/** Build the import object for `module`; every bucket_v0 function not written here answers -UNSUPPORTED. */
function makeImports(module, memoryRef, host) {
  const bucket = {};
  const sig = {};
  for (const imp of WebAssembly.Module.imports(module)) {
    if (imp.module === 'bucket_v0') sig[imp.name] = true;
  }
  const mem = () => new DataView(memoryRef.buffer);
  const bytes = () => new Uint8Array(memoryRef.buffer);
  const text = (p, n) => new TextDecoder().decode(bytes().slice(p, p + n));
  const impl = {
    api_version: () => 3,
    caps: () => host.caps,
    launch_reason: () => 0,
    cpu_count: () => 4,
    time_now_us: () => nowUs(),
    limit_get: (which) => ({ 0: 255n, 1: 64n << 20n, 2: 512n << 20n, 3: 256n, 4: 256n, 5: 1n << 20n, 6: 8n << 20n, 7: 4096n, 8: 64n }[which] ?? -1n),
    log: (level, p, n) => { if (host.log) host.log(level, text(p, n)); },
    exit: () => {},
    restart: () => 0,
    events_wake: () => 0,
    power_inhibit: () => 0,
    cursor_set: () => 0,
    canvas_info: (out) => {
      const v = mem();
      v.setUint32(out, 32, true); v.setUint32(out + 4, 320, true); v.setUint32(out + 8, 180, true);
      v.setFloat32(out + 12, 1.0, true); v.setUint32(out + 16, 1, true); v.setUint32(out + 20, 60000, true);
      v.setUint32(out + 24, 2, true);
      return 0;
    },
    canvas_present: (_p, len, _x, _y, w, h) => { host.presents++; return len === 320 * 180 * 4 ? 0 : ERR.INVALID; },
    kv_load: () => ERR.NOT_FOUND,
    kv_store: () => 0,
    kv_flush: () => { host.flushes++; return 0; },
    now_playing_clear: () => 0,
    events_wait: (buf, max, timeoutUs) => {
      host.waits++;
      const v = mem();
      const rec = (kind, fill) => { const o = buf + 64 * host.queued++; v.setUint16(o, kind, true); fill(o); };
      host.queued = 0;
      if (host.waits === 1) rec(EV.RESIZE, (o) => { v.setUint32(o + 16, 320, true); v.setUint32(o + 20, 180, true); v.setFloat32(o + 24, 1.0, true); });
      else if (performance.now() - host.started > host.seconds * 1000) rec(EV.TERMINATE, (o) => v.setUint32(o + 16, 1000, true));
      else {
        const ms = Math.min(Number(timeoutUs) / 1000, 20);
        if (ms > 0) Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
      }
      return host.queued;
    },
    thread_spawn: (arg) => {
      if (!host.spawn) return ERR.UNSUPPORTED;
      return host.spawn(arg);
    },
    thread_yield: () => {},
    thread_priority: () => 0,
  };
  for (const name of Object.keys(sig)) {
    bucket[name] = impl[name] ?? (() => (name === 'time_now_us' || name === 'caps' || name === 'limit_get' || name.endsWith('_us') || name === 'file_size' || name === 'library_listing' ? BigInt(ERR.UNSUPPORTED) : ERR.UNSUPPORTED));
  }
  return { bucket_v0: bucket };
}

function memoryFor(module) {
  const imp = WebAssembly.Module.imports(module).find((i) => i.module === 'env' && i.name === 'memory');
  if (!imp) return null;
  return new WebAssembly.Memory({ initial: 2048, maximum: 32768, shared: true });
}

async function instantiate(module, memory, host) {
  const ref = { buffer: null, get buffer() { return (memory ?? instance.exports.memory).buffer; } };
  const imports = makeImports(module, ref, host);
  if (memory) imports.env = { memory };
  const instance = await WebAssembly.instantiate(module, imports);
  return instance;
}

if (!isMainThread) {
  // A thread the module started: a new instance of the same module on the same memory.
  const { module, memory, tid, arg } = workerData;
  const host = { caps: 0n, spawn: null, started: performance.now(), seconds: 0, presents: 0, flushes: 0, waits: 0, queued: 0 };
  const instance = await instantiate(module, memory, host);
  instance.exports.bucket_thread_start(tid, arg);
  parentPort.postMessage('done');
} else {
  const args = process.argv.slice(2);
  const file = args.find((a) => !a.startsWith('--') && !/^\d+(\.\d+)?$/.test(a));
  const opt = (name, dflt) => { const i = args.indexOf(name); return i >= 0 ? args[i + 1] : dflt; };
  const threads = args.includes('--threads');
  const smokeThreads = Number(opt('--smoke-threads', 0));
  const seconds = Number(opt('--seconds', 1));
  if (!file) { console.error('usage: bucket-node-smoke.mjs <module.wasm> [--threads] [--smoke-threads N] [--seconds S]'); process.exit(2); }

  let failed = 0;
  const check = (ok, what) => { console.log(`${ok ? 'ok  ' : 'FAIL'} ${what}`); if (!ok) failed++; };

  const module = await WebAssembly.compile(readFileSync(file));
  const memory = memoryFor(module);
  check(!!memory === threads, threads ? 'the threads build imports a shared memory' : 'the plain build has its own memory');
  const workers = [];
  let nextTid = 0;
  const host = {
    caps: CAPS.CANVAS | CAPS.AUDIO_OUT | CAPS.NOW_PLAYING | CAPS.LIBRARY | (threads ? CAPS.THREADS | CAPS.ATOMICS : 0n),
    started: performance.now(), seconds, presents: 0, flushes: 0, waits: 0, queued: 0,
    log: (level, t) => { if (level <= 1) console.log(`     log(${level}): ${t}`); },
    spawn: (arg) => {
      const tid = ++nextTid;
      workers.push(new Worker(fileURLToPath(import.meta.url), { workerData: { module, memory, tid, arg } }));
      return tid;
    },
  };
  const instance = await instantiate(module, memory, host);
  const ex = instance.exports;
  check(typeof ex.bucket_main === 'function' && typeof ex.bucket_save_state === 'function' && typeof ex.bucket_restore_state === 'function', 'bucket_main, bucket_save_state and bucket_restore_state are exported');
  check(threads === (typeof ex.bucket_thread_start === 'function'), threads ? 'bucket_thread_start is exported' : 'no thread entry in the plain build');

  if (threads && smokeThreads > 0 && typeof ex.bucket_smoke_threads === 'function') {
    const got = ex.bucket_smoke_threads(smokeThreads);
    check(got === smokeThreads, `${smokeThreads} threads ran on their own stacks and TLS blocks while the main thread worked (result ${got})`);
    check(workers.length === smokeThreads, `thread_spawn was called ${workers.length} times`);
    for (const w of workers) w.terminate();
    process.exit(failed ? 1 : 0);
  }

  let status;
  try {
    status = ex.bucket_main();
    check(status === 0, `bucket_main ran and returned ${status} after TERMINATE`);
  } catch (e) {
    check(false, `bucket_main trapped: ${e}`);
  }
  check(host.presents > 0, `the app presented ${host.presents} frames to the canvas`);
  check(host.waits > 3, `the loop waited for events ${host.waits} times`);
  check(host.flushes >= 1, `kv_flush was called on TERMINATE (${host.flushes})`);
  check(ex.bucket_save_state(0, 0) === 0, 'bucket_save_state reports no state');
  check(ex.bucket_restore_state(0, 0) === 0, 'bucket_restore_state takes nothing');
  for (const w of workers) w.terminate();
  process.exit(failed ? 1 : 0);
}
