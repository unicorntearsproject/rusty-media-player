//! `cargo xtask bucket`: build the wasm module(s) of Rusty Wave for Rusty Bucket and pack `Rusty Wave.bucket`
//! (a ZIP: `manifest.toml`, the modules, the icon, `CHECKSUMS`, optionally `SIGNATURE`; see `docs/developer/bucket-format.md` in
//! `../rust-os`), check the modules' imports against the documented App API, and run them in Node against a stand-in host.
//! `cargo xtask bucket-smoke` runs the Node checks alone on quick builds, `cargo xtask bucket-e2e` runs the packed app in the Bucket
//! Simulator when one is installed.
//!
//! Builds, in `target/bucket-*` (one target directory each, so none invalidates another):
//!
//! | file | what | needs |
//! | --- | --- | --- |
//! | `app.wasm` | baseline: Wasm 2.0, no SIMD, no threads; runs on every engine | stable or nightly |
//! | `app.simd.wasm` | + SIMD128 (`--simd`) | |
//! | `app.threads.wasm` | + SIMD128, atomics, shared memory imported as `env.memory` | nightly + `rust-src` (`-Z build-std`) |
//!
//! Nothing is deleted recursively: staging is overwritten in place.
use bucket_v0_sys::inspect::{ImportKind, check_imports, parse_module};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const USAGE: &str = "usage: cargo xtask bucket [--no-opt] [--no-threads] [--simd] [--no-smoke] [--version V]
       cargo xtask bucket-smoke
       cargo xtask bucket-e2e [--sim PATH] [--bucket FILE] [--only NAME...] [-v]

bucket         build the baseline (and with --simd the SIMD) module and the threads module, check their imports against the documented
               function set, pack target/bucket/Rusty Wave.bucket, and run the modules in Node (tools/bucket-node-smoke.mjs)
  --no-opt     skip wasm-opt          --no-threads  baseline only (no nightly needed)
  --simd       also build app.simd.wasm   --no-smoke  skip the Node checks   --version V  stamp V instead of Cargo's version
  Signing hook: RVP_BUCKET_SIGN_CMD is run (sh -c) with BUCKET_CHECKSUMS (the file to sign) and BUCKET_SIGNATURE (the file to write)
  set; whatever it writes becomes SIGNATURE. Without it the bundle is unsigned (it runs on the interpreter with a warning).
bucket-smoke   the Node checks on quick builds without decoders: lifecycle of both builds and the thread start-up contract
bucket-e2e     run the scenarios of xtask/src/bucket_e2e.rs on Rusty Wave.bucket in the Bucket Simulator, headless (--sim, $BUCKET_SIM,
               ../rust-os/tools/bucket-sim/target/release/bucket-sim, or PATH; skipped when there is none). --only runs the scenarios
               whose name contains NAME; -v prints the app's log. Screenshots go to target/bucket-e2e/<scenario>/";

const APP_ID: &str = "io.github.idometeor.RustyWave";
const MODULE_CRATE: &str = "rvp-wave-bucket";
const WASM_NAME: &str = "rvp_wave_bucket.wasm";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn out_dir() -> PathBuf {
    root().join("target/bucket")
}

/// One wasm variant.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Variant {
    Baseline,
    Simd,
    Threads,
}

impl Variant {
    fn file(self) -> &'static str {
        match self {
            Variant::Baseline => "app.wasm",
            Variant::Simd => "app.simd.wasm",
            Variant::Threads => "app.threads.wasm",
        }
    }

    fn target_dir(self) -> &'static str {
        match self {
            Variant::Baseline => "target/bucket-build",
            Variant::Simd => "target/bucket-build-simd",
            Variant::Threads => "target/bucket-build-threads",
        }
    }

    /// What the manifest says this build requires.
    fn requires(self) -> &'static [&'static str] {
        match self {
            Variant::Baseline => &[],
            Variant::Simd => &["simd128"],
            Variant::Threads => &["simd128", "threads"],
        }
    }

    fn rustflags(self) -> String {
        match self {
            Variant::Baseline => String::new(),
            Variant::Simd => "-C target-feature=+simd128".into(),
            // Shared memory and atomics need the standard library rebuilt with them; the memory is imported as `env.memory` (the
            // linker's default) with the module's own initial size and a 2 GiB maximum (the manifest's `memory_max_mb`).
            Variant::Threads => "-C target-feature=+simd128,+atomics,+bulk-memory,+mutable-globals \
                 -C link-arg=--shared-memory -C link-arg=--max-memory=2147483648 -C link-arg=--import-memory"
                .into(),
        }
    }
}

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().is_ok_and(|o| o.status.success())
}

fn run(cmd: &mut Command, what: &str) -> Result<(), String> {
    let st = cmd.status().map_err(|e| format!("{what}: {e}"))?;
    st.success().then_some(()).ok_or_else(|| format!("{what} failed"))
}

/// `cargo build --release` of the module crate for one variant; returns the path of the wasm.
fn build(variant: Variant, features: &[&str], target_dir: &str) -> Result<PathBuf, String> {
    let dir = root().join(target_dir);
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.current_dir(root()).args([
        "build",
        "--release",
        "--target",
        "wasm32-unknown-unknown",
        "-p",
        MODULE_CRATE,
        "--target-dir",
    ]);
    cmd.arg(&dir);
    if !features.is_empty() {
        cmd.args(["--no-default-features", "--features", &features.join(",")]);
    }
    if variant == Variant::Threads {
        cmd.args(["-Z", "build-std=std,panic_abort"]);
    }
    cmd.env_remove("CARGO_ENCODED_RUSTFLAGS").env("RUSTFLAGS", variant.rustflags());
    println!(
        "+ cargo build --release --target wasm32-unknown-unknown -p {MODULE_CRATE}{} [{}]",
        if features.is_empty() { "" } else { " (no decoders)" },
        variant.file()
    );
    run(
        &mut cmd,
        &format!(
            "building {}{}",
            variant.file(),
            if variant == Variant::Threads {
                " (needs a nightly toolchain with the rust-src component)"
            } else {
                ""
            }
        ),
    )?;
    Ok(dir.join("wasm32-unknown-unknown/release").join(WASM_NAME))
}

/// Shrink a module with wasm-opt, enabling the features the build uses.
fn optimise(wasm: &Path, variant: Variant) {
    if !have("wasm-opt") {
        eprintln!("xtask: wasm-opt not installed, skipping (cargo install wasm-opt)");
        return;
    }
    let tmp = wasm.with_extension("opt.wasm");
    let mut cmd = Command::new("wasm-opt");
    cmd.args([
        "-O2",
        "--enable-sign-ext",
        "--enable-mutable-globals",
        "--enable-nontrapping-float-to-int",
        "--enable-multivalue",
        "--enable-reference-types",
        "--enable-bulk-memory",
    ]);
    if variant != Variant::Baseline {
        cmd.arg("--enable-simd");
    }
    if variant == Variant::Threads {
        cmd.arg("--enable-threads");
    }
    println!("+ wasm-opt -O2 {}", wasm.display());
    cmd.arg("-o").arg(&tmp).arg(wasm);
    match cmd.status() {
        Ok(s) if s.success() => {
            if let Err(e) = fs::rename(&tmp, wasm) {
                eprintln!("xtask: cannot replace the module: {e}");
            }
        }
        _ => eprintln!("xtask: wasm-opt failed, keeping the unoptimised module"),
    }
}

/// Check a module against the documented API: imports exactly the documented functions (and `env.memory` shared for threads), exports
/// the entry points. Returns a one-line summary.
fn verify_module(path: &Path, variant: Variant) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let info = parse_module(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let rep = check_imports(&info);
    if !rep.ok() {
        return Err(format!(
            "{} does not match the documented App API:\n  {}",
            variant.file(),
            rep.problems.join("\n  ")
        ));
    }
    let threads = variant == Variant::Threads;
    if rep.shared_memory != threads {
        return Err(format!(
            "{}: {}",
            variant.file(),
            if threads {
                "the threads build must import a shared memory"
            } else {
                "only the threads build may import a shared memory"
            }
        ));
    }
    if let Some(imp) = info.imports.iter().find(|i| i.module == "env" && i.name == "memory")
        && let ImportKind::Memory { max, .. } = &imp.kind
        && max.is_none_or(|m| m > 32768)
    {
        return Err(format!(
            "{}: the shared memory must declare a maximum of at most 2 GiB (32768 pages), found {max:?}",
            variant.file()
        ));
    }
    for name in ["bucket_main", "bucket_save_state", "bucket_restore_state", "memory"] {
        if !info.exports.iter().any(|(n, _)| n == name) && !(name == "memory" && threads) {
            return Err(format!("{}: `{name}` is not exported", variant.file()));
        }
    }
    let has_thread_entry = info.exports.iter().any(|(n, _)| n == "bucket_thread_start");
    if has_thread_entry != threads {
        return Err(format!(
            "{}: bucket_thread_start must be exported by the threads build and only by it",
            variant.file()
        ));
    }
    Ok(format!(
        "{}: {} KiB, imports exactly the {} documented functions{}",
        variant.file(),
        bytes.len() / 1024,
        rep.bucket_functions,
        if threads { " and a shared memory" } else { "" }
    ))
}

fn sha256(path: &Path) -> Result<String, String> {
    let out = Command::new("sha256sum").arg(path).output().map_err(|e| format!("sha256sum: {e}"))?;
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(String::from)
        .ok_or_else(|| "sha256sum gave nothing".into())
}

fn cargo_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// The manifest text: the template with the version and the builds that were made.
pub fn manifest_text(template: &str, version: &str, builds: &[Variant2]) -> Result<String, String> {
    if !template.contains("# @BUILDS@") || !template.contains("@VERSION@") {
        return Err("the manifest template lost its @VERSION@ or @BUILDS@ marker".into());
    }
    // Most capable first: the OS picks the best build it can load.
    let mut blocks = Vec::new();
    for b in builds {
        let mut s = format!("[[builds]]\nfile = \"{}\"", b.file);
        if !b.requires.is_empty() {
            s += &format!(
                "\nrequires = [{}]",
                b.requires.iter().map(|r| format!("\"{r}\"")).collect::<Vec<_>>().join(", ")
            );
        } else {
            s += "                      # baseline: runs on every engine";
        }
        blocks.push(s);
    }
    Ok(template.replace("@VERSION@", version).replace("# @BUILDS@", &blocks.join("\n")))
}

/// A build as the manifest lists it.
pub struct Variant2 {
    pub file: &'static str,
    pub requires: &'static [&'static str],
}

fn zip_entries(stage: &Path, zip: &Path, names: &[String]) -> Result<(), String> {
    if zip.exists() {
        fs::remove_file(zip).map_err(|e| e.to_string())?;
    }
    if have("zip") {
        // -X: no extra file attributes; -D: no directory entries; deflate (the default) is allowed by the format.
        return run(
            Command::new("zip").current_dir(stage).args(["-q", "-X", "-D"]).arg(zip).args(names),
            "zip",
        );
    }
    let py = "import sys, zipfile\nz = zipfile.ZipFile(sys.argv[1], 'w', zipfile.ZIP_DEFLATED)\nfor n in sys.argv[2:]:\n    z.write(n, n)\nz.close()\n";
    let python = if have("python3") { "python3" } else { "python" };
    run(Command::new(python).current_dir(stage).arg("-c").arg(py).arg(zip).args(names), "python zip")
}

/// Check the package: it is a valid ZIP with every entry the manifest needs, and `CHECKSUMS` matches the contents.
fn verify_package(zip: &Path, names: &[String]) -> Result<(), String> {
    if have("unzip") {
        run(Command::new("unzip").args(["-tq"]).arg(zip).stdout(std::process::Stdio::null()), "unzip -t")?;
        let list = Command::new("unzip").args(["-Z1"]).arg(zip).output().map_err(|e| e.to_string())?;
        let listed: Vec<String> = String::from_utf8_lossy(&list.stdout).lines().map(String::from).collect();
        for n in names {
            if !listed.contains(n) {
                return Err(format!("`{n}` is missing from the package"));
            }
        }
        if listed.first().map(String::as_str) != Some("manifest.toml") {
            return Err("manifest.toml must be the first entry".into());
        }
        if listed.iter().any(|n| n.starts_with('/') || n.contains("..") || n.ends_with('/')) {
            return Err("the package has an absolute path, a `..` or a directory entry".into());
        }
        // The checksums describe what is in the archive.
        let sums = Command::new("unzip")
            .args(["-p"])
            .arg(zip)
            .arg("CHECKSUMS")
            .output()
            .map_err(|e| e.to_string())?;
        for line in String::from_utf8_lossy(&sums.stdout).lines() {
            let (hash, name) = line.split_once("  ").ok_or("a CHECKSUMS line is malformed")?;
            let data =
                Command::new("unzip").args(["-p"]).arg(zip).arg(name).output().map_err(|e| e.to_string())?;
            let tmp = out_dir().join(".verify");
            fs::write(&tmp, &data.stdout).map_err(|e| e.to_string())?;
            if sha256(&tmp)? != hash {
                return Err(format!("CHECKSUMS does not match `{name}`"));
            }
        }
    }
    Ok(())
}

/// `cargo xtask bucket`.
pub fn run_bucket(args: &[String]) -> Result<(), String> {
    let flag = |f: &str| args.iter().any(|a| a == f);
    let version = args
        .iter()
        .position(|a| a == "--version")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(cargo_version);
    let mut variants = vec![Variant::Baseline];
    if flag("--simd") {
        variants.push(Variant::Simd);
    }
    if !flag("--no-threads") {
        variants.push(Variant::Threads);
    }
    let out = out_dir();
    let stage = out.join("stage");
    fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    // A module left from an earlier run with other options must not end up in the package.
    for f in fs::read_dir(&stage).map_err(|e| e.to_string())?.flatten() {
        if f.path().is_file() {
            fs::remove_file(f.path()).map_err(|e| e.to_string())?;
        }
    }
    let mut built = Vec::new();
    for v in &variants {
        let wasm = build(*v, &[], v.target_dir())?;
        if !flag("--no-opt") {
            optimise(&wasm, *v);
        }
        println!("{}", verify_module(&wasm, *v)?);
        fs::copy(&wasm, stage.join(v.file())).map_err(|e| format!("copying {}: {e}", wasm.display()))?;
        built.push(*v);
    }
    // The manifest lists the most capable build first.
    let mut listed: Vec<Variant2> =
        built.iter().rev().map(|v| Variant2 { file: v.file(), requires: v.requires() }).collect();
    listed.sort_by_key(|b| std::cmp::Reverse(b.requires.len()));
    let template = fs::read_to_string(root().join("packaging/bucket/manifest.toml.in"))
        .map_err(|e| format!("manifest template: {e}"))?;
    fs::write(stage.join("manifest.toml"), manifest_text(&template, &version, &listed)?)
        .map_err(|e| e.to_string())?;
    // The icons: 256 px is required, 64 and 32 are optional.
    let icons = root().join("packaging/icons/hicolor");
    for (size, name) in [(256, "icon.png"), (64, "icon-64.png"), (32, "icon-32.png")] {
        let src = icons.join(format!("{size}x{size}/apps/{APP_ID}.png"));
        fs::copy(&src, stage.join(name)).map_err(|e| format!("{}: {e}", src.display()))?;
    }
    // CHECKSUMS: SHA-256 of every other entry; SIGNATURE (the publisher's Ed25519 signature over CHECKSUMS) from the hook, if any.
    let mut names: Vec<String> = fs::read_dir(&stage)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "CHECKSUMS" && n != "SIGNATURE")
        .collect();
    names.sort();
    let mut sums = String::new();
    for n in &names {
        sums += &format!("{}  {n}\n", sha256(&stage.join(n))?);
    }
    fs::write(stage.join("CHECKSUMS"), sums).map_err(|e| e.to_string())?;
    let mut entries: Vec<String> = vec!["manifest.toml".into()];
    entries.extend(names.iter().filter(|n| *n != "manifest.toml").cloned());
    entries.push("CHECKSUMS".into());
    if let Ok(cmd) = std::env::var("RVP_BUCKET_SIGN_CMD") {
        let sig = stage.join("SIGNATURE");
        println!("+ {cmd}");
        run(
            Command::new("sh")
                .arg("-c")
                .arg(&cmd)
                .env("BUCKET_CHECKSUMS", stage.join("CHECKSUMS"))
                .env("BUCKET_SIGNATURE", &sig),
            "the signing hook",
        )?;
        if !sig.exists() {
            return Err("the signing hook did not write BUCKET_SIGNATURE".into());
        }
        entries.push("SIGNATURE".into());
    } else {
        println!("(unsigned: set RVP_BUCKET_SIGN_CMD to sign CHECKSUMS)");
    }
    let zip = out.join("Rusty Wave.bucket");
    zip_entries(&stage, &zip, &entries)?;
    verify_package(&zip, &entries)?;
    println!(
        "{} ({} KiB, version {version}, builds: {})",
        zip.display(),
        fs::metadata(&zip).map_or(0, |m| m.len()) / 1024,
        built.iter().map(|v| v.file()).collect::<Vec<_>>().join(", ")
    );
    if !flag("--no-smoke") {
        smoke_modules(&built.iter().map(|v| (*v, stage.join(v.file()))).collect::<Vec<_>>())?;
        smoke_threads()?;
    }
    Ok(())
}

fn node_smoke(module: &Path, extra: &[&str]) -> Result<(), String> {
    if !have("node") {
        return Err("node is needed for the module checks (use --no-smoke to skip them)".into());
    }
    run(
        Command::new("node").arg(root().join("tools/bucket-node-smoke.mjs")).arg(module).args(extra),
        "the Node check",
    )
}

/// The lifecycle of the modules that were packed, against the stand-in host.
fn smoke_modules(modules: &[(Variant, PathBuf)]) -> Result<(), String> {
    for (v, path) in modules {
        println!("== {} in Node", v.file());
        let extra: &[&str] =
            if *v == Variant::Threads { &["--threads", "--seconds", "1"] } else { &["--seconds", "1"] };
        node_smoke(path, extra)?;
    }
    Ok(())
}

/// The thread start-up contract: a build with the `smoke` export, no decoders (quick), run with real Workers.
fn smoke_threads() -> Result<(), String> {
    println!("== the thread start-up contract in Node");
    let wasm = build(Variant::Threads, &["smoke"], "target/bucket-smoke-threads")?;
    node_smoke(&wasm, &["--threads", "--smoke-threads", "4"])
}

/// `cargo xtask bucket-smoke`.
pub fn run_smoke() -> Result<(), String> {
    for (v, dir) in
        [(Variant::Baseline, "target/bucket-smoke-plain"), (Variant::Threads, "target/bucket-smoke-threads")]
    {
        let wasm = build(v, &["smoke"], dir)?;
        println!("{}", verify_module(&wasm, v)?);
        println!("== {} in Node", v.file());
        let extra: &[&str] = if v == Variant::Threads {
            &["--threads", "--seconds", "1", "--smoke-threads", "4"]
        } else {
            &["--seconds", "1"]
        };
        node_smoke(&wasm, extra)?;
    }
    Ok(())
}

/// The simulator binary: `--sim`, `$BUCKET_SIM`, the Rusty Bucket checkout's release or debug build, or `bucket-sim` on the PATH.
fn find_sim(arg: Option<&str>) -> Option<PathBuf> {
    let candidates: Vec<PathBuf> = arg
        .map(PathBuf::from)
        .into_iter()
        .chain(std::env::var_os("BUCKET_SIM").map(PathBuf::from))
        .chain([
            root().join("../rust-os/tools/bucket-sim/target/release/bucket-sim"),
            root().join("../rust-os/tools/bucket-sim/target/debug/bucket-sim"),
        ])
        .collect();
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .or_else(|| have("bucket-sim").then(|| PathBuf::from("bucket-sim")))
}

/// `cargo xtask bucket-e2e [--sim PATH] [--bucket FILE] [--only NAME...] [-v] [--only NAME...] [-v]`: run the packed app through the scenarios of
/// `bucket_e2e.rs` in the Bucket Simulator (headless, virtual time). Skipped, successfully, when there is no simulator.
pub fn run_e2e(args: &[String]) -> Result<(), String> {
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let Some(sim) = find_sim(opt("--sim").as_deref()) else {
        println!(
            "skipped: no bucket-sim found (--sim PATH, $BUCKET_SIM, ../rust-os/tools/bucket-sim/target/release/bucket-sim, or on the PATH;\n  build it with `cargo build --release --manifest-path ../rust-os/tools/bucket-sim/Cargo.toml`)"
        );
        return Ok(());
    };
    let bucket = opt("--bucket").map_or_else(|| out_dir().join("Rusty Wave.bucket"), PathBuf::from);
    if !bucket.exists() {
        return Err(format!("{} does not exist: run `cargo xtask bucket` first", bucket.display()));
    }
    let fixtures = root().join("target/fixtures");
    if !fixtures.join("h264_aac.mp4").exists() || !fixtures.join("library/music").exists() {
        println!("== making the fixtures");
        crate::fixtures(&[])?;
    }
    // Scratch data directories: fresh and in the system temp directory (never deleted from here).
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let tmp = std::env::temp_dir().join(format!("rvp-bucket-e2e-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let out = root().join("target/bucket-e2e");
    fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let only: Vec<String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| *i > 0 && args[*i - 1] == "--only" && !a.starts_with('-'))
        .map(|(_, a)| a.clone())
        .collect();
    println!("{} runs {} (screenshots in {})", sim.display(), bucket.display(), out.display());
    let ctx = crate::bucket_e2e::Ctx {
        sim,
        bucket: fs::canonicalize(&bucket).unwrap_or(bucket),
        fixtures: fs::canonicalize(&fixtures).unwrap_or(fixtures),
        tmp,
        out,
        verbose: args.iter().any(|a| a == "-v"),
    };
    match crate::bucket_e2e::run_all(&ctx, &only) {
        0 => Ok(()),
        n => Err(format!("{n} Simulator scenario(s) failed")),
    }
}
