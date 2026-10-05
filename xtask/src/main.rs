//! Repo automation. Run as `cargo xtask <command>`.
mod theme;

use std::process::{Command, ExitCode};

const USAGE: &str = "usage: cargo xtask <command>

  theme [--sync]   regenerate crates/theme/src/tokens.rs from crates/theme/tokens/*.css
                   (--sync first refreshes the CSS snapshot from the design system;
                   set UT_DESIGN_SYSTEM to override its path)
  check            cargo check for the host, and for wasm32 / no_std targets where applicable
  fixtures [dir]   generate ffmpeg test media into target/fixtures (tools/gen-fixtures.sh)
  web | serve | licenses   not implemented yet (see docs/PLAN.md)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("theme") => theme::run(args.iter().any(|a| a == "--sync")),
        Some("check") => check(),
        Some("fixtures") => fixtures(&args[1..]),
        Some(cmd @ ("web" | "serve" | "licenses")) => Err(format!("`{cmd}` is not implemented yet")),
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

fn fixtures(extra: &[String]) -> Result<(), String> {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/gen-fixtures.sh");
    let status = Command::new("bash").arg(script).args(extra).status().map_err(|e| e.to_string())?;
    status.success().then_some(()).ok_or_else(|| "fixture generation failed".to_string())
}

fn cargo(args: &[&str]) -> Result<(), String> {
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
    "rvp-host",
    "rvp-demux",
    "rvp-codec-audio",
    "rvp-codec-h264",
    "rvp-codec-av1",
    "rvp-codec-vp9",
    "rvp-subs",
    "rvp-player",
    "rvp-ui",
    "rvp-host-web",
    "rvp-host-rb",
];

/// Crates that promise `no_std + alloc` and must build for a target with no std at all.
const NO_STD_CRATES: &[&str] =
    &["theme", "rvp-core", "rvp-host", "rvp-demux", "rvp-codec-h264", "rvp-subs", "rvp-player", "rvp-ui"];

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
