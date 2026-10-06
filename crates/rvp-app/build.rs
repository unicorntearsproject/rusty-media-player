//! Build script: the commit this build was made from, for the About page (`RVP_BUILD_COMMIT`, a short hash; the environment
//! variable of that name wins, which is how a build from a source archive without `.git` says it).
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=RVP_BUILD_COMMIT");
    println!("cargo:rerun-if-changed=../../.git/logs/HEAD");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    let commit = std::env::var("RVP_BUILD_COMMIT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "--short=10", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=RVP_BUILD_COMMIT={commit}");
}
