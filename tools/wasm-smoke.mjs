// Run the rvp-wasm-smoke module in Node: decode an AV1 or H.264 file inside WebAssembly and print "<frames> <hash>".
// usage: node tools/wasm-smoke.mjs <module.wasm> <file> [--bench N]
// With --bench N the file is decoded N times and the best time is printed to stderr (the module is warmed up first).
import { readFileSync } from "node:fs";
const args = process.argv.slice(2);
const [wasmPath, filePath] = args;
const bench = args.includes("--bench") ? Number(args[args.indexOf("--bench") + 1] || 3) : 0;
const { instance } = await WebAssembly.instantiate(readFileSync(wasmPath), {});
const x = instance.exports;
const file = readFileSync(filePath);
function run() {
  const ptr = x.smoke_alloc(file.length);
  new Uint8Array(x.memory.buffer, ptr, file.length).set(file);
  return x.smoke_run(ptr, file.length);
}
let n = run();
if (n < 0) { console.error("decode failed"); process.exit(1); }
if (bench) {
  let best = Infinity;
  for (let i = 0; i < bench; i++) {
    const t = performance.now();
    run();
    best = Math.min(best, performance.now() - t);
  }
  console.error(`wasm: ${n} frames, best ${best.toFixed(0)} ms, ${(best / n).toFixed(2)} ms/frame, ${(n / best * 1000).toFixed(1)} fps`);
}
const hash = (BigInt(x.smoke_hash_hi() >>> 0) << 32n) | BigInt(x.smoke_hash_lo() >>> 0);
console.log(`${n} ${hash.toString(16).padStart(16, "0")}`);
