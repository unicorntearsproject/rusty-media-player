//! The wasm module links against exactly the documented `bucket_v0` function set.
//!
//! Builds the module for `wasm32-unknown-unknown` without decoders (fast; the imports do not depend on them) into its own target
//! directory, parses the import and export sections and checks them against `bucket_v0_sys::FUNCTIONS`: every import is a documented
//! function with its documented signature, every documented function is imported, nothing else is, and the entry points are exported.
//! `cargo xtask bucket` does the same check on the full module.
use bucket_v0_sys::inspect::{check_imports, parse_module};
use std::path::PathBuf;
use std::process::Command;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn the_module_imports_exactly_the_documented_api() {
    let target = workspace().join("target/bucket-test");
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "build",
            "--target",
            "wasm32-unknown-unknown",
            "-p",
            "rvp-wave-bucket",
            "--no-default-features",
            "--target-dir",
        ])
        .arg(&target)
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .output()
        .expect("run cargo");
    assert!(
        out.status.success(),
        "building the module for wasm32 failed (is the wasm32-unknown-unknown target installed?):\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let wasm = std::fs::read(target.join("wasm32-unknown-unknown/debug/rvp_wave_bucket.wasm"))
        .expect("the built module");
    let info = parse_module(&wasm).expect("a valid module");
    let report = check_imports(&info);
    assert!(report.ok(), "the imports drifted from the documented API:\n{}", report.problems.join("\n"));
    assert!(!report.shared_memory, "the plain build must not import a shared memory");
    assert_eq!(report.bucket_functions, bucket_v0_sys::FUNCTIONS.len());
    for name in ["bucket_main", "bucket_save_state", "bucket_restore_state", "memory"] {
        assert!(info.exports.iter().any(|(n, _)| n == name), "`{name}` is not exported: {:?}", info.exports);
    }
    assert!(
        !info.exports.iter().any(|(n, _)| n == "bucket_thread_start"),
        "the plain build has no threads, so no thread entry"
    );
}
