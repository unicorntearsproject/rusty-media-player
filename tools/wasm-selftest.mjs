// Run the SIMD128 self-tests (vector kernels vs scalar twins) and, with --bench, the picture-path timings.
// usage: node tools/wasm-selftest.mjs <rvp_wasm_smoke.wasm> [--bench]
import { readFileSync } from "node:fs";
const [wasmPath] = process.argv.slice(2);
const { instance } = await WebAssembly.instantiate(readFileSync(wasmPath), {});
const x = instance.exports;
const bad = x.smoke_selftest();
const parts = ["colour+scaler", "h264", "vp9"].map((n, i) => `${n}=${x.smoke_selftest_part(i)}`).join(" ");
console.log(`simd=${x.smoke_has_simd()} selftest mismatches=${bad} (${parts})`);
if (bad) process.exit(1);
if (process.argv.includes("--bench")) {
  for (const [w, h, dw, dh] of [[1920, 1080, 1280, 720], [1920, 1080, 1920, 1080], [1280, 720, 1280, 720]]) {
    const out = [];
    for (const [name, mode] of [["convert", 1], ["scale", 2], ["both", 3]]) {
      x.smoke_present_bench(w, h, dw, dh, 2, mode);
      let best = Infinity;
      for (let i = 0; i < 5; i++) {
        const t = performance.now();
        x.smoke_present_bench(w, h, dw, dh, 5, mode);
        best = Math.min(best, (performance.now() - t) / 5);
      }
      out.push(`${name} ${best.toFixed(2)} ms`);
    }
    console.log(`present ${w}x${h} -> ${dw}x${dh}: ${out.join(", ")}`);
  }
}
