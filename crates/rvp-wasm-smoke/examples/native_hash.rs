//! Native counterpart of the wasm smoke test: prints `<frames> <hash>` for a file.
fn main() {
    let path = std::env::args().nth(1).expect("usage: native_hash <file.webm>");
    let (n, h) = rvp_wasm_smoke::decode_av1(std::fs::read(path).unwrap()).unwrap();
    println!("{n} {h:016x}");
}
