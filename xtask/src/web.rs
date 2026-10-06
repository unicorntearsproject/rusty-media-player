//! `cargo xtask web` (build the browser player) and `cargo xtask serve` (a tiny static file server).
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Where the built page lives.
pub fn out_dir() -> PathBuf {
    root().join("target/web")
}

/// Remove the files directly inside `dir` (wasm-bindgen output is flat), leaving the directory itself: no recursive delete.
fn clear_files(dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.path().is_file() {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().is_ok_and(|o| o.status.success())
}

/// Build `rvp-host-web` for wasm32, run wasm-bindgen, optionally wasm-opt, and copy `web/` next to it. With `threads`
/// the shared-memory variant (atomics, worker threads) is built as well, into `pkg-mt`; the page uses it when it can.
pub fn build(no_opt: bool, threads: bool) -> Result<(), String> {
    let root = root();
    let out = out_dir();
    let size = build_variant(&root, &out.join("pkg"), "target/web-build", false, no_opt)?;
    let mut note = format!("{} KiB wasm", size / 1024);
    if threads {
        let size = build_variant(&root, &out.join("pkg-mt"), "target/web-threads-build", true, no_opt)?;
        note += &format!(", {} KiB wasm with threads", size / 1024);
    } else {
        // A stale threaded build must not be picked up by a page that was rebuilt without it.
        clear_files(&out.join("pkg-mt"));
    }
    for entry in std::fs::read_dir(root.join("web")).map_err(|e| e.to_string())? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if p.is_file() {
            std::fs::copy(&p, out.join(p.file_name().unwrap_or_default())).map_err(|e| e.to_string())?;
        } else if p.is_dir() {
            // One level of subdirectories (icons/): files only.
            let to = out.join(p.file_name().unwrap_or_default());
            std::fs::create_dir_all(&to).map_err(|e| e.to_string())?;
            for f in std::fs::read_dir(&p).map_err(|e| e.to_string())?.flatten() {
                if f.path().is_file() {
                    std::fs::copy(f.path(), to.join(f.file_name())).map_err(|e| e.to_string())?;
                }
            }
        }
    }
    let version = stamp_service_worker(&out)?;
    println!("built {} ({note}; service worker {version})", out.display());
    Ok(())
}

/// Every file below `dir` as a path relative to it, sorted.
fn walk_files(dir: &Path, base: &Path, out: &mut Vec<String>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk_files(&p, base, out);
            } else if let Ok(rel) = p.strip_prefix(base) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
}

/// Fill in the placeholders of `sw.js`: the list of files the page is made of (the service worker precaches them) and a version
/// made of the Cargo version and a hash of those files, so any change to the page is a new service worker.
fn stamp_service_worker(out: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    walk_files(out, out, &mut files);
    files.retain(|f| f != "sw.js" && !f.ends_with(".map"));
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for f in &files {
        for b in f.bytes().chain(std::fs::read(out.join(f)).map_err(|e| e.to_string())?) {
            hash = (hash ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    let version = format!("{}-{:010x}", env!("CARGO_PKG_VERSION"), hash & 0xff_ffff_ffff);
    let mut urls: Vec<String> = vec!["./".into()];
    urls.extend(files.iter().map(|f| format!("./{f}")));
    let list = format!("[{}]", urls.iter().map(|u| format!("{u:?}")).collect::<Vec<_>>().join(", "));
    let sw = std::fs::read_to_string(out.join("sw.js")).map_err(|e| e.to_string())?;
    let sw = sw.replace("__RVP_VERSION__", &version).replace("__RVP_PRECACHE__", &list);
    std::fs::write(out.join("sw.js"), sw).map_err(|e| e.to_string())?;
    Ok(version)
}

/// One wasm variant: returns the size of the final module. The page needs a modern browser anyway, so every variant is
/// built with wasm SIMD128. A separate target dir per variant keeps them from invalidating each other and the plain
/// `cargo check` / `cargo test` caches.
fn build_variant(
    root: &Path,
    pkg: &Path,
    target_dir: &str,
    threads: bool,
    no_opt: bool,
) -> Result<u64, String> {
    let build_dir = root.join(target_dir);
    let mut flags = std::env::var("RUSTFLAGS").unwrap_or_default();
    flags.push_str(" -C target-feature=+simd128");
    let mut args: Vec<&str> = vec![
        "build",
        "--release",
        "--target",
        "wasm32-unknown-unknown",
        "-p",
        "rvp-host-web",
        "--target-dir",
    ];
    let dir_s = build_dir.to_string_lossy().into_owned();
    args.push(&dir_s);
    if threads {
        // Shared memory and atomics need the standard library rebuilt with them (nightly + rust-src), and the linker
        // told to share and import the memory and export the TLS setup wasm-bindgen threads on top of.
        flags.push_str(
            " -C target-feature=+atomics,+bulk-memory,+mutable-globals \
             -C link-arg=--shared-memory -C link-arg=--max-memory=2147483648 -C link-arg=--import-memory \
             -C link-arg=--export=__wasm_init_tls -C link-arg=--export=__tls_size \
             -C link-arg=--export=__tls_align -C link-arg=--export=__tls_base",
        );
        args.extend(["-Z", "build-std=std,panic_abort"]);
    }
    println!(
        "+ cargo build --release --target wasm32-unknown-unknown -p rvp-host-web (simd128{})",
        if threads { ", threads" } else { "" }
    );
    let st = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(&args)
        .env("RUSTFLAGS", flags.split_whitespace().collect::<Vec<_>>().join(" "))
        .status()
        .map_err(|e| e.to_string())?;
    if !st.success() {
        return Err(format!(
            "cargo build for the web player{} failed{}",
            if threads { " (threads variant)" } else { "" },
            if threads { " (it needs a nightly toolchain with the rust-src component)" } else { "" }
        ));
    }
    let wasm = build_dir.join("wasm32-unknown-unknown/release/rvp_host_web.wasm");
    clear_files(pkg);
    std::fs::create_dir_all(pkg).map_err(|e| e.to_string())?;
    if !have("wasm-bindgen") {
        return Err("wasm-bindgen CLI not found: `cargo install wasm-bindgen-cli --version <the wasm-bindgen crate version in Cargo.lock>`".into());
    }
    println!("+ wasm-bindgen --target web");
    let st = Command::new("wasm-bindgen")
        .args(["--target", "web", "--no-typescript", "--out-name", "rvp", "--out-dir"])
        .arg(pkg)
        .arg(&wasm)
        .status()
        .map_err(|e| e.to_string())?;
    if !st.success() {
        return Err(
            "wasm-bindgen failed (its version must match the wasm-bindgen crate in Cargo.lock)".into()
        );
    }
    let bg = pkg.join("rvp_bg.wasm");
    if !no_opt && have("wasm-opt") && !wasm_opt_too_old() {
        println!("+ wasm-opt -O2");
        let tmp = pkg.join("rvp_bg.opt.wasm");
        let mut cmd = Command::new("wasm-opt");
        cmd.args([
            "-O2",
            "--enable-simd",
            "--enable-bulk-memory",
            "--enable-sign-ext",
            "--enable-mutable-globals",
            "--enable-nontrapping-float-to-int",
            "--enable-multivalue",
            "--enable-reference-types",
        ]);
        if threads {
            cmd.arg("--enable-threads");
        }
        let st = cmd.arg("-o").arg(&tmp).arg(&bg).status().map_err(|e| e.to_string())?;
        if st.success() {
            std::fs::rename(&tmp, &bg).map_err(|e| e.to_string())?;
        } else {
            eprintln!("xtask: wasm-opt failed, keeping the unoptimised module");
        }
    } else if !no_opt {
        eprintln!("xtask: no usable wasm-opt (missing, or binaryen older than 116), skipping the size optimisation");
    }
    Ok(std::fs::metadata(&bg).map(|m| m.len()).unwrap_or(0))
}

/// binaryen older than 116 (Ubuntu 22.04 ships 105) optimises our module into one that does not start in Chrome, so it is not used.
fn wasm_opt_too_old() -> bool {
    let out = Command::new("wasm-opt").arg("--version").output().ok();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    // "wasm-opt version 123 (version_123)"
    let version = text.split_whitespace().skip_while(|w| *w != "version").nth(1).and_then(|v| v.parse::<u32>().ok());
    version.is_none_or(|v| v < 116)
}

fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "wasm" => "application/wasm",
        "json" => "application/json",
        "webmanifest" => "application/manifest+json",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

fn respond(mut s: TcpStream, dir: &Path) -> std::io::Result<()> {
    let mut line = String::new();
    let mut reader = BufReader::new(s.try_clone()?);
    reader.read_line(&mut line)?;
    // Drain the headers.
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
    }
    let mut it = line.split_whitespace();
    let (method, target) = (it.next().unwrap_or(""), it.next().unwrap_or("/"));
    let rel = target.split(['?', '#']).next().unwrap_or("/").trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    let safe = !rel.split('/').any(|c| c == ".." || c.contains('\\'));
    let path = dir.join(rel);
    let (status, body, ctype) = match (safe, std::fs::read(&path)) {
        (true, Ok(b)) => ("200 OK", b, mime(&path)),
        _ => ("404 Not Found", b"not found".to_vec(), "text/plain"),
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         Cross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Embedder-Policy: require-corp\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes())?;
    if method != "HEAD" {
        s.write_all(&body)?;
    }
    s.flush()
}

/// Serve `target/web` (or `--dir`) on `--port` (default 8080) until killed.
pub fn serve(args: &[String]) -> Result<(), String> {
    let mut port = 8080u16;
    let mut dir = out_dir();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--port" => port = it.next().and_then(|v| v.parse().ok()).ok_or("--port needs a number")?,
            "--dir" => dir = PathBuf::from(it.next().ok_or("--dir needs a path")?),
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    if !dir.join("index.html").exists() {
        return Err(format!("{} has no index.html: run `cargo xtask web` first", dir.display()));
    }
    let listener =
        TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("bind 127.0.0.1:{port}: {e}"))?;
    println!("serving {} on http://127.0.0.1:{port}/", dir.display());
    for conn in listener.incoming().flatten() {
        let dir = dir.clone();
        std::thread::spawn(move || {
            let _ = respond(conn, &dir);
        });
    }
    Ok(())
}

/// Build the page, make fixtures, and run the Playwright suite in `tests/e2e`.
/// Options: `--update-golden` rewrites the golden screenshot; `--screenshots` regenerates `docs/screenshots`;
/// `--threads` also builds the shared-memory variant and runs the suite a second time with worker threads on;
/// anything after `--` goes to `playwright test`.
pub fn e2e(args: &[String]) -> Result<(), String> {
    let root = root();
    let e2e = root.join("tests/e2e");
    let threads = args.iter().any(|a| a == "--threads");
    build(false, threads)?;
    if !root.join("target/fixtures/.done").exists() {
        crate::fixtures(&[])?;
    }
    crate::fixture_set("audio")?;
    crate::fixture_set("levels")?;
    let npm = |args: &[&str]| -> Result<(), String> {
        println!("+ (tests/e2e) {}", args.join(" "));
        let st = Command::new(args[0])
            .args(&args[1..])
            .current_dir(&e2e)
            .status()
            .map_err(|e| format!("{}: {e}", args[0]))?;
        st.success().then_some(()).ok_or_else(|| format!("`{}` failed", args.join(" ")))
    };
    if !e2e.join("node_modules").exists() {
        npm(&["npm", "install", "--no-audit", "--no-fund"])?;
    }
    npm(&["npx", "playwright", "install", "chromium"])?;
    let mut env: Vec<(&str, &str)> = Vec::new();
    let mut extra: Vec<&str> = Vec::new();
    let mut rest = false;
    for a in args {
        match a.as_str() {
            "--update-golden" => env.push(("UPDATE_GOLDEN", "1")),
            "--screenshots" => {
                env.push(("RVP_SCREENSHOTS", "1"));
                extra.extend(["-g", "screenshots"]);
            }
            "--threads" => {}
            "--" => rest = true,
            other if rest => extra.push(other),
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    // With the threaded variant built, run once on the single-threaded build (the baseline, selected by a cookie the
    // page honours) and once with threads.
    let passes: &[(&str, &str)] =
        if threads { &[("single-threaded", "0"), ("threaded", "1")] } else { &[("single-threaded", "0")] };
    for (name, on) in passes {
        let mut cmd = Command::new("npx");
        cmd.args(["playwright", "test"]).args(&extra).current_dir(&e2e);
        for (k, v) in &env {
            cmd.env(k, v);
        }
        cmd.env("RVP_E2E_THREADS", on);
        println!("+ (tests/e2e) npx playwright test [{name}]");
        let st = cmd.status().map_err(|e| e.to_string())?;
        if !st.success() {
            return Err(format!("the browser tests failed ({name})"));
        }
    }
    Ok(())
}

/// Play the 1080p30 streams of `cargo xtask perf-fixtures` in the browser and report the dropped frames.
/// Options: `--secs N` per stream (default 60), `--only NAME` (a part of the file name), `--single` for the
/// single-threaded baseline only, `--both` for both builds.
pub fn perf(args: &[String]) -> Result<(), String> {
    let root = root();
    let e2e = root.join("tests/e2e");
    let (mut secs, mut only) = ("60".to_string(), String::new());
    let (mut single, mut both) = (false, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--secs" => secs = it.next().ok_or("--secs needs a number")?.clone(),
            "--only" => only = it.next().ok_or("--only needs a name")?.clone(),
            "--single" => single = true,
            "--both" => both = true,
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    build(false, true)?;
    crate::fixture_set("perf")?;
    if !e2e.join("node_modules").exists() {
        let st = Command::new("npm")
            .args(["install", "--no-audit", "--no-fund"])
            .current_dir(&e2e)
            .status()
            .map_err(|e| e.to_string())?;
        if !st.success() {
            return Err("npm install failed".into());
        }
    }
    let passes: &[(&str, &str)] = if both {
        &[("single-threaded", "threads=0"), ("threaded", "")]
    } else if single {
        &[("single-threaded", "threads=0")]
    } else {
        &[("threaded", "")]
    };
    let mut failed: Vec<&str> = Vec::new();
    for (name, query) in passes {
        println!("+ (tests/e2e) npx playwright test perf.spec.js [{name}]");
        let st = Command::new("npx")
            .args(["playwright", "test", "perf.spec.js"])
            .current_dir(&e2e)
            .env("RVP_PERF", "1")
            .env("RVP_PERF_SECS", &secs)
            .env("RVP_PERF_ONLY", &only)
            .env("RVP_PERF_QUERY", query)
            .env("RVP_E2E_THREADS", if query.is_empty() { "1" } else { "0" })
            .status()
            .map_err(|e| e.to_string())?;
        if !st.success() {
            failed.push(*name);
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("the real-time check failed ({})", failed.join(", ")))
    }
}
