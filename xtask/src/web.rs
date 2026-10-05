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

/// Build `rvp-host-web` for wasm32, run wasm-bindgen, optionally wasm-opt, and copy `web/` next to it.
pub fn build(no_opt: bool) -> Result<(), String> {
    let root = root();
    // The page needs a modern browser anyway, so build with wasm SIMD128: the compiler then vectorises the
    // pixel loops (picture scaling, scrims, YUV conversion) and the codecs. A separate target dir keeps this
    // build from invalidating the plain `cargo check` / `cargo test` caches.
    let build_dir = root.join("target/web-build");
    let mut flags = std::env::var("RUSTFLAGS").unwrap_or_default();
    flags.push_str(" -C target-feature=+simd128");
    println!("+ cargo build --release --target wasm32-unknown-unknown -p rvp-host-web (simd128)");
    let st = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "build",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
            "-p",
            "rvp-host-web",
            "--target-dir",
        ])
        .arg(&build_dir)
        .env("RUSTFLAGS", flags.trim())
        .status()
        .map_err(|e| e.to_string())?;
    if !st.success() {
        return Err("cargo build for the web player failed".into());
    }
    let wasm = build_dir.join("wasm32-unknown-unknown/release/rvp_host_web.wasm");
    let out = out_dir();
    let pkg = out.join("pkg");
    std::fs::create_dir_all(&pkg).map_err(|e| e.to_string())?;
    if !have("wasm-bindgen") {
        return Err("wasm-bindgen CLI not found: `cargo install wasm-bindgen-cli --version <the wasm-bindgen crate version in Cargo.lock>`".into());
    }
    println!("+ wasm-bindgen --target web");
    let st = Command::new("wasm-bindgen")
        .args(["--target", "web", "--no-typescript", "--out-name", "rvp", "--out-dir"])
        .arg(&pkg)
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
        let st = Command::new("wasm-opt")
            .args([
                "-O2",
                "--enable-simd",
                "--enable-bulk-memory",
                "--enable-sign-ext",
                "--enable-mutable-globals",
                "--enable-nontrapping-float-to-int",
                "--enable-multivalue",
                "--enable-reference-types",
                "-o",
            ])
            .arg(&tmp)
            .arg(&bg)
            .status()
            .map_err(|e| e.to_string())?;
        if st.success() {
            std::fs::rename(&tmp, &bg).map_err(|e| e.to_string())?;
        } else {
            eprintln!("xtask: wasm-opt failed, keeping the unoptimised module");
        }
    } else if !no_opt {
        eprintln!("xtask: wasm-opt not installed, skipping (cargo install wasm-opt)");
    }
    for entry in std::fs::read_dir(root.join("web")).map_err(|e| e.to_string())? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if p.is_file() {
            std::fs::copy(&p, out.join(p.file_name().unwrap_or_default())).map_err(|e| e.to_string())?;
        }
    }
    let size = std::fs::metadata(&bg).map(|m| m.len()).unwrap_or(0);
    println!("built {} ({} KiB wasm)", out.display(), size / 1024);
    Ok(())
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
/// anything after `--` goes to `playwright test`.
pub fn e2e(args: &[String]) -> Result<(), String> {
    let root = root();
    let e2e = root.join("tests/e2e");
    build(false)?;
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
    let mut cmd = Command::new("npx");
    cmd.args(["playwright", "test"]).current_dir(&e2e);
    let mut rest = false;
    for a in args {
        match a.as_str() {
            "--update-golden" => {
                cmd.env("UPDATE_GOLDEN", "1");
            }
            "--screenshots" => {
                cmd.env("RVP_SCREENSHOTS", "1").args(["-g", "screenshots"]);
            }
            "--" => rest = true,
            other if rest => {
                cmd.arg(other);
            }
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    println!("+ (tests/e2e) npx playwright test");
    let st = cmd.status().map_err(|e| e.to_string())?;
    st.success().then_some(()).ok_or_else(|| "the browser tests failed".to_string())
}
