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
        let _ = std::fs::remove_dir_all(out.join("pkg-mt"));
    }
    for entry in std::fs::read_dir(root.join("web")).map_err(|e| e.to_string())? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if p.is_file() {
            std::fs::copy(&p, out.join(p.file_name().unwrap_or_default())).map_err(|e| e.to_string())?;
        }
    }
    println!("built {} ({note})", out.display());
    Ok(())
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
    let _ = std::fs::remove_dir_all(pkg);
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
    if !no_opt && have("wasm-opt") {
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
        eprintln!("xtask: wasm-opt not installed, skipping (cargo install wasm-opt)");
    }
    Ok(std::fs::metadata(&bg).map(|m| m.len()).unwrap_or(0))
}

fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "wasm" => "application/wasm",
        "json" => "application/json",
        "png" => "image/png",
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
