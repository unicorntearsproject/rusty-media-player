//! Repo automation. Run as `cargo xtask <command>`.
mod theme;
mod web;

use std::process::{Command, ExitCode};

const USAGE: &str = "usage: cargo xtask <command>

  theme [--sync]   regenerate crates/theme/src/tokens.rs from crates/theme/tokens/*.css
                   (--sync first refreshes the CSS snapshot from the design system;
                   set UT_DESIGN_SYSTEM to override its path)
  check            cargo check for the host, and for wasm32 / no_std targets where applicable
  wasm-smoke       decode AV1, H.264 and VP9 fixtures inside WebAssembly (Node) and compare with the native decoder
  fixtures [dir]   generate ffmpeg test media into target/fixtures (tools/gen-fixtures.sh)
  web [--no-opt] [--threads]   build the browser player into target/web (wasm32 release, wasm-bindgen, wasm-opt if installed);
                   --threads also builds the shared-memory variant (pkg-mt: atomics, worker threads; needs nightly + rust-src)
  serve [--port N] [--dir D]   serve target/web (default port 8080) with the headers a wasm page likes
  e2e [--update-golden] [--screenshots] [--threads] [-- args]
                   build the page, make fixtures and run the Playwright suite (tests/e2e);
                   --screenshots regenerates docs/screenshots
  licenses         not implemented yet (see docs/PLAN.md)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("theme") => theme::run(args.iter().any(|a| a == "--sync")),
        Some("check") => check(),
        Some("fixtures") => fixtures(&args[1..]),
        Some("wasm-smoke") => wasm_smoke(),
        Some("web") => {
            web::build(args.iter().any(|a| a == "--no-opt"), args.iter().any(|a| a == "--threads"))
        }
        Some("serve") => web::serve(&args[1..]),
        Some("e2e") => web::e2e(&args[1..]),
        Some(cmd @ "licenses") => Err(format!("`{cmd}` is not implemented yet")),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

pub(crate) fn fixtures(extra: &[String]) -> Result<(), String> {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/gen-fixtures.sh");
    let status = Command::new("bash").arg(script).args(extra).status().map_err(|e| e.to_string())?;
    status.success().then_some(()).ok_or_else(|| "fixture generation failed".to_string())
}

/// Make one fixture set (see `tools/gen-fixtures.sh`) if its marker file is missing.
pub(crate) fn fixture_set(set: &str) -> Result<(), String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    if root.join("target/fixtures").join(set).join(".done").exists() {
        return Ok(());
    }
    let script = root.join("tools/gen-fixtures.sh");
    let status = Command::new("bash").arg(script).env("RVP_FIXTURE_SET", set).status().map_err(|e| e.to_string())?;
    status.success().then_some(()).ok_or_else(|| format!("generating the `{set}` fixtures failed"))
}

/// Build the smoke module for wasm32 (plain, and with SIMD128), run it in Node on AV1, H.264 and VP9 fixtures, and
/// require the same frame count and hash as the native build from both; the SIMD build also runs the self-tests that
/// compare every vector kernel with its scalar twin.
fn wasm_smoke() -> Result<(), String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    cargo(&["build", "--release", "--target", "wasm32-unknown-unknown", "-p", "rvp-wasm-smoke"])?;
    let wasm = root.join("target/wasm32-unknown-unknown/release/rvp_wasm_smoke.wasm");
    println!("+ cargo build --release --target wasm32-unknown-unknown -p rvp-wasm-smoke (simd128)");
    let st = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "build",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
            "-p",
            "rvp-wasm-smoke",
            "--target-dir",
        ])
        .arg(root.join("target/wasm-simd"))
        .env("RUSTFLAGS", "-C target-feature=+simd128")
        .status()
        .map_err(|e| e.to_string())?;
    if !st.success() {
        return Err("the SIMD128 smoke build failed".into());
    }
    let wasm_simd = root.join("target/wasm-simd/wasm32-unknown-unknown/release/rvp_wasm_smoke.wasm");
    let st = Command::new("node")
        .arg(root.join("tools/wasm-selftest.mjs"))
        .arg(&wasm_simd)
        .status()
        .map_err(|e| e.to_string())?;
    if !st.success() {
        return Err("a SIMD128 kernel disagrees with its scalar reference".into());
    }
    // AV1 and H.264 (CAVLC Baseline, CABAC Main with B-frames, High with 8x8 transform and scaling matrices).
    for name in [
        "av1_opus.webm",
        "av1_10bit.webm",
        "h264/i_base_cif.mp4",
        "h264/b_cavlc_pyramid.mp4",
        "h264/c_main_b.mp4",
        "h264/h_high_cqm_jvt.mp4",
        "vp9/s_352x288.webm",
        "vp9/t_altref.webm",
        "vp9/s_odd_327x245.webm",
        "vp9/r_keyframe.webm",
        "vp9/t_tiles4.webm",
        "vp9/s_odd_130x66.webm",
        "vp9/t_lossless.webm",
        "vp9/x_profile2_10bit.webm",
        "vp9/t_aq_cyclic.webm",
        "h264/h_high.mp4",
        "h264/h_high_odd.mp4",
        "h264/c_main_odd.mp4",
        "h264/p_base_odd.mp4",
        "h264/h_high_slices.mp4",
    ] {
        let file = root.join("target/fixtures").join(name);
        if !file.exists() {
            fixtures(&[])?;
        }
        let run = |cmd: &mut Command| -> Result<String, String> {
            let out = cmd.output().map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).into_owned());
            }
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        };
        let native = run(Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args(["run", "-q", "--release", "-p", "rvp-wasm-smoke", "--example", "native_hash", "--"])
            .arg(&file))?;
        let wasm_out =
            run(Command::new("node").arg(root.join("tools/wasm-smoke.mjs")).arg(&wasm).arg(&file))?;
        let simd_out =
            run(Command::new("node").arg(root.join("tools/wasm-smoke.mjs")).arg(&wasm_simd).arg(&file))?;
        println!("{name}: native `{native}`, wasm `{wasm_out}`, wasm simd128 `{simd_out}`");
        if native != wasm_out || native != simd_out {
            return Err(format!("{name}: wasm and native decoders disagree"));
        }
    }
    Ok(())
}

pub(crate) fn cargo(args: &[&str]) -> Result<(), String> {
    println!("+ cargo {}", args.join(" "));
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(args)
        .status()
        .map_err(|e| e.to_string())?;
    status.success().then_some(()).ok_or_else(|| format!("cargo {} failed", args.join(" ")))
}

/// Crates that must build for wasm32-unknown-unknown (everything except the native-only host and xtask).
const WASM_CRATES: &[&str] = &[
    "theme",
    "rvp-core",
    "rvp-par",
    "rvp-host",
    "rvp-demux",
    "rvp-codec-audio",
    "rvp-codec-h264",
    "rvp-codec-av1",
    "rvp-codec-vp9",
    "rvp-subs",
    "rvp-viz",
    "rvp-player",
    "rvp-ui",
    "rvp-app",
    "rvp-host-web",
    "rvp-host-rb",
];

/// Crates that promise `no_std + alloc` and must build for a target with no std at all.
const NO_STD_CRATES: &[&str] = &[
    "theme",
    "rvp-core",
    "rvp-host",
    "rvp-demux",
    "rvp-codec-h264",
    "rvp-subs",
    "rvp-viz",
    "rvp-player",
    "rvp-ui",
    "rvp-app",
];

fn check() -> Result<(), String> {
    cargo(&["check", "--workspace", "--all-targets"])?;
    let mut wasm = vec!["check", "--target", "wasm32-unknown-unknown"];
    for c in WASM_CRATES {
        wasm.extend(["-p", c]);
    }
    cargo(&wasm)?;
    let mut bare = vec!["check", "--target", "x86_64-unknown-none"];
    for c in NO_STD_CRATES {
        bare.extend(["-p", c]);
    }
    cargo(&bare)
}
