// Run the rvp-wasm-smoke module in Node: decode an AV1 WebM inside WebAssembly and print "<frames> <hash>".
// usage: node tools/wasm-smoke.mjs <module.wasm> <file.webm>
import { readFileSync } from "node:fs";
const [wasmPath, filePath] = process.argv.slice(2);
const { instance } = await WebAssembly.instantiate(readFileSync(wasmPath), {});
const x = instance.exports;
const file = readFileSync(filePath);
const ptr = x.smoke_alloc(file.length);
new Uint8Array(x.memory.buffer, ptr, file.length).set(file);
const n = x.smoke_run(ptr, file.length);
if (n < 0) { console.error("decode failed"); process.exit(1); }
const hash = (BigInt(x.smoke_hash_hi() >>> 0) << 32n) | BigInt(x.smoke_hash_lo() >>> 0);
console.log(`${n} ${hash.toString(16).padStart(16, "0")}`);
