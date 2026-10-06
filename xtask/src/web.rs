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
    build_as(no_opt, threads, None)
}

/// [`build`], with the version `build-info.json` reports (a dry run stamps another one than Cargo's).
pub fn build_as(no_opt: bool, threads: bool, version: Option<&str>) -> Result<(), String> {
    let root = root();
    let out = out_dir();
    // Files of an earlier build (hashed names change with the content) must not linger next to the new ones.
    clear_files(&out);
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
    let renamed = hash_assets(&out)?;
    let sw = stamp_service_worker(&out)?;
    write_build_info(&root, &out, version.unwrap_or(env!("CARGO_PKG_VERSION")), &renamed)?;
    println!("built {} ({note}; {} hashed names; service worker {sw})", out.display(), renamed.len());
    Ok(())
}

// ---- content-hashed names ----------------------------------------------------------------------------------------------------------

/// Files that get a hash in their name: the scripts, the style sheet and the wasm modules with their glue. `index.html`, `sw.js`, the
/// manifest, the icons and `build-info.json` keep their names (they are what the browser asks for first, or what must not change).
fn is_hashed(rel: &str) -> bool {
    rel != "sw.js" && (rel.ends_with(".js") || rel.ends_with(".css") || rel.ends_with(".wasm"))
}

fn dir_of(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(d, _)| d)
}

/// How a file in `from_dir` names `target` (both relative to the page root): the shortest relative path.
fn relative_to(from_dir: &str, target: &str) -> String {
    if from_dir.is_empty() {
        return target.to_string();
    }
    match target.strip_prefix(&format!("{from_dir}/")) {
        Some(rest) => rest.to_string(),
        None => format!("../{target}"),
    }
}

/// Byte offsets where `needle` appears in `text` as a whole file reference: right after a quote, backtick, bracket or space (with or
/// without a leading `./`), and not followed by more of a name.
fn find_refs(text: &str, needle: &str) -> Vec<usize> {
    let b = text.as_bytes();
    let name = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b'/');
    let mut v = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find(needle) {
        let at = from + i;
        from = at + needle.len();
        let end = at + needle.len();
        if end < b.len() && (b[end].is_ascii_alphanumeric() || matches!(b[end], b'_' | b'-')) {
            continue;
        }
        // The character before, past an optional `./`.
        let mut start = at;
        if start >= 2 && &b[start - 2..start] == b"./" {
            start -= 2;
        }
        if start > 0 && name(b[start - 1]) {
            continue;
        }
        v.push(at);
    }
    v
}

/// Give the page's scripts, styles and wasm names with a hash of their content (and of everything they load), rewrite every reference,
/// and return `(original, hashed)` names. A file's hash covers its own bytes and those of every file reachable from it, so a cycle
/// (a worker and the module that starts it) is no obstacle and an unchanged file keeps its name between builds.
fn hash_assets(out: &Path) -> Result<Vec<(String, String)>, String> {
    let mut all = Vec::new();
    walk_files(out, out, &mut all);
    let targets: Vec<String> = all.iter().filter(|f| is_hashed(f)).cloned().collect();
    let is_text = |f: &str| !f.ends_with(".wasm") && !f.ends_with(".png") && !f.ends_with(".ico");
    let mut orig = std::collections::BTreeMap::new();
    for f in &all {
        orig.insert(f.clone(), std::fs::read(out.join(f)).map_err(|e| e.to_string())?);
    }
    // Who loads whom.
    let mut loads: std::collections::BTreeMap<&str, Vec<&str>> = std::collections::BTreeMap::new();
    for f in all.iter().filter(|f| is_text(f)) {
        let text = String::from_utf8_lossy(&orig[f]).into_owned();
        for t in targets.iter().filter(|t| *t != f) {
            if !find_refs(&text, &relative_to(dir_of(f), t)).is_empty() {
                loads.entry(f.as_str()).or_default().push(t.as_str());
            }
        }
    }
    let mut renamed = Vec::new();
    let mut new_name = std::collections::BTreeMap::new();
    for t in &targets {
        // The closure of what `t` loads.
        let mut seen = std::collections::BTreeSet::new();
        let mut todo = vec![t.as_str()];
        while let Some(f) = todo.pop() {
            if seen.insert(f) {
                todo.extend(loads.get(f).into_iter().flatten().copied());
            }
        }
        let mut h = Sha256::new();
        for f in &seen {
            h.update(f.as_bytes());
            h.update(&orig[*f]);
        }
        let hex = h.finish_hex();
        let (stem, ext) = t.rsplit_once('.').ok_or("a hashed file without an extension")?;
        let n = format!("{stem}.{}.{ext}", &hex[..8]);
        new_name.insert(t.clone(), n.clone());
        renamed.push((t.clone(), n));
    }
    // Rewrite references everywhere (the unhashed index.html and manifest too), then move the files to their new names.
    for f in all.iter().filter(|f| is_text(f)) {
        let mut text = String::from_utf8_lossy(&orig[f]).into_owned();
        // Longest names first, so `rvp_bg.wasm` is not caught inside a longer name.
        let mut order: Vec<&String> = targets.iter().filter(|t| *t != f).collect();
        order.sort_by_key(|t| std::cmp::Reverse(t.len()));
        for t in order {
            let (from, to) = (relative_to(dir_of(f), t), relative_to(dir_of(f), &new_name[t]));
            let mut at = find_refs(&text, &from);
            at.sort_unstable_by(|a, b| b.cmp(a));
            for i in at {
                text.replace_range(i..i + from.len(), &to);
            }
        }
        let dest = new_name.get(f).unwrap_or(f);
        std::fs::write(out.join(dest), text).map_err(|e| e.to_string())?;
        if dest != f {
            std::fs::remove_file(out.join(f)).map_err(|e| e.to_string())?;
        }
    }
    for t in targets.iter().filter(|t| !is_text(t)) {
        std::fs::rename(out.join(t), out.join(&new_name[t])).map_err(|e| e.to_string())?;
    }
    renamed.sort();
    Ok(renamed)
}

/// Seconds since the Unix epoch as an RFC 3339 UTC time (`2026-10-06T04:44:19Z`).
fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// `build-info.json`, at the root of the page and of the web zip: what this build is and the SHA-256 and size of every other file, so a
/// deployment can be checked against it (`immutable` is true for the files whose names carry a content hash: a cache may keep them for ever).
/// Schema 1: `name`, `version`, `commit` (the full sha), `built` (RFC 3339 UTC), `threads` (whether
/// `pkg-mt/` is there) and `files`. `SOURCE_DATE_EPOCH` fixes `built` for a reproducible build.
fn write_build_info(
    root: &Path,
    out: &Path,
    version: &str,
    renamed: &[(String, String)],
) -> Result<(), String> {
    let commit = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    let secs = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64)
        });
    let mut files = Vec::new();
    walk_files(out, out, &mut files);
    files.retain(|f| f != "build-info.json" && !f.ends_with(".map"));
    let threads = files.iter().any(|f| f.starts_with("pkg-mt/") && f.ends_with(".wasm"));
    let mut json = String::from("{\n  \"schema\": 1,\n  \"name\": \"rusty-wave-web\",\n");
    json += &format!(
        "  \"version\": {version:?},\n  \"commit\": {commit:?},\n  \"built\": \"{}\",\n",
        rfc3339(secs)
    );
    json += &format!("  \"threads\": {threads},\n  \"files\": {{\n");
    for (i, f) in files.iter().enumerate() {
        let bytes = std::fs::read(out.join(f)).map_err(|e| e.to_string())?;
        let mut h = Sha256::new();
        h.update(&bytes);
        json += &format!(
            "    {f:?}: {{ \"sha256\": \"{}\", \"size\": {}, \"immutable\": {} }}{}\n",
            h.finish_hex(),
            bytes.len(),
            renamed.iter().any(|(_, to)| to == f),
            if i + 1 < files.len() { "," } else { "" }
        );
    }
    json += "  }\n}\n";
    std::fs::write(out.join("build-info.json"), json).map_err(|e| e.to_string())
}

/// SHA-256 (FIPS 180-4), enough for hashing the build's files without a tool or a crate.
struct Sha256 {
    h: [u32; 8],
    buf: Vec<u8>,
    len: u64,
}

impl Sha256 {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];

    fn new() -> Self {
        Self {
            h: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buf: Vec::new(),
            len: 0,
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.len += data.len() as u64;
        self.feed(data);
    }

    fn feed(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
        let whole = self.buf.len() / 64 * 64;
        let blocks: Vec<u8> = self.buf.drain(..whole).collect();
        for block in blocks.chunks_exact(64) {
            self.block(block);
        }
    }

    fn block(&mut self, b: &[u8]) {
        let mut w = [0u32; 64];
        for (i, c) in b.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        #[allow(clippy::needless_range_loop)]
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = self.h;
        #[allow(clippy::needless_range_loop)]
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(Self::K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for (h, v) in self.h.iter_mut().zip(v) {
            *h = h.wrapping_add(v);
        }
    }

    fn finish_hex(mut self) -> String {
        let bits = self.len * 8;
        let mut tail = vec![0x80u8];
        while (self.buf.len() + tail.len()) % 64 != 56 {
            tail.push(0);
        }
        tail.extend_from_slice(&bits.to_be_bytes());
        self.feed(&tail);
        self.h.iter().map(|x| format!("{x:08x}")).collect()
    }
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

/// Fill in the placeholders of `sw.js`: the files the page is made of (the service worker precaches them) and a version made of the Cargo
/// version and a hash of those files, so any change to the page is a new service worker. The files are three lists: the page's own
/// (`__RVP_PRECACHE__`) and the two wasm builds (`__RVP_PKG__`, `__RVP_PKG_MT__`), of which the worker fetches only the one the browser
/// can run.
fn stamp_service_worker(out: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    walk_files(out, out, &mut files);
    files.retain(|f| f != "sw.js" && f != "build-info.json" && !f.ends_with(".map"));
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for f in &files {
        for b in f.bytes().chain(std::fs::read(out.join(f)).map_err(|e| e.to_string())?) {
            hash = (hash ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    let version = format!("{}-{:010x}", env!("CARGO_PKG_VERSION"), hash & 0xff_ffff_ffff);
    let list = |urls: Vec<String>| {
        format!("[{}]", urls.iter().map(|u| format!("{u:?}")).collect::<Vec<_>>().join(", "))
    };
    let mut page: Vec<String> = vec!["./".into()];
    page.extend(
        files
            .iter()
            .filter(|f| !f.starts_with("pkg/") && !f.starts_with("pkg-mt/"))
            .map(|f| format!("./{f}")),
    );
    let pkg = files.iter().filter(|f| f.starts_with("pkg/")).map(|f| format!("./{f}")).collect();
    let mt = files.iter().filter(|f| f.starts_with("pkg-mt/")).map(|f| format!("./{f}")).collect();
    let sw = std::fs::read_to_string(out.join("sw.js")).map_err(|e| e.to_string())?;
    let sw = sw
        .replace("__RVP_VERSION__", &version)
        .replace("__RVP_PRECACHE__", &list(page))
        .replace("__RVP_PKG_MT__", &list(mt))
        .replace("__RVP_PKG__", &list(pkg));
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
        eprintln!(
            "xtask: no usable wasm-opt (missing, or binaryen older than 116), skipping the size optimisation"
        );
    }
    Ok(std::fs::metadata(&bg).map(|m| m.len()).unwrap_or(0))
}

/// binaryen older than 116 (Ubuntu 22.04 ships 105) optimises our module into one that does not start in Chrome, so it is not used.
fn wasm_opt_too_old() -> bool {
    let out = Command::new("wasm-opt").arg("--version").output().ok();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    // "wasm-opt version 123 (version_123)"
    let version =
        text.split_whitespace().skip_while(|w| *w != "version").nth(1).and_then(|v| v.parse::<u32>().ok());
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
/// The page's server (`xtask serve`) as a child process that lives for the whole run and is stopped when this is dropped.
struct Server(Option<std::process::Child>);

impl Server {
    fn start(port: &str) -> Result<Server, String> {
        // Already one on that port (a developer's own): use it and leave it alone.
        let addr = format!("127.0.0.1:{port}");
        if std::net::TcpStream::connect(&addr).is_ok() {
            return Ok(Server(None));
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let child = Command::new(exe)
            .args(["serve", "--port", port])
            .current_dir(root())
            .stdout(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start the page's server: {e}"))?;
        let mut server = Server(Some(child));
        for _ in 0..100 {
            if std::net::TcpStream::connect(&addr).is_ok() {
                return Ok(server);
            }
            if let Some(c) = &mut server.0 {
                if let Ok(Some(st)) = c.try_wait() {
                    return Err(format!("the page's server stopped at once ({st})"));
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        Err("the page's server did not come up".into())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(c) = &mut self.0 {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

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
    // One server for every pass (a pass that starts its own stops it on the way out, and the next one meets a closed port).
    let port = std::env::var("RVP_E2E_PORT").unwrap_or_else(|_| "4173".into());
    let _server = Server::start(&port)?;
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
            .env("RVP_E2E_PORT", &port)
            .env("RVP_E2E_OUTPUT", root.join("target/perf-out").join(name))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(s: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(s);
        h.finish_hex()
    }

    #[test]
    fn sha256_matches_the_known_answers() {
        assert_eq!(sha(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let long = vec![b'a'; 1000];
        assert_eq!(sha(&long), "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3");
    }

    #[test]
    fn times_are_rfc3339_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_791_261_859), "2026-10-06T04:44:19Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn references_are_found_as_whole_names() {
        let t = "import x from \"./audio.js\"; new URL('audio-worklet.js', import.meta.url); \"my-audio.js\" `audio.js` audio.jsx";
        let at = find_refs(t, "audio.js");
        assert_eq!(at.len(), 2, "{at:?}");
        assert_eq!(&t[at[0]..at[0] + 8], "audio.js");
        assert_eq!(relative_to("pkg", "pkg/rvp_bg.wasm"), "rvp_bg.wasm");
        assert_eq!(relative_to("", "pkg/rvp.js"), "pkg/rvp.js");
        assert_eq!(relative_to("pkg", "main.js"), "../main.js");
    }

    #[test]
    fn hashing_renames_rewrites_and_survives_a_cycle() {
        let dir = std::env::temp_dir().join(format!("rvp-hash-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("pkg")).unwrap();
        let put = |f: &str, s: &str| std::fs::write(dir.join(f), s).unwrap();
        put("index.html", "<link href=\"style.css\"><script src=\"main.js\"></script>");
        put("style.css", "body{}");
        put("main.js", "import './threads.js'; import('./pkg/rvp.js');");
        put("threads.js", "new Worker(new URL('./worker.js', import.meta.url));");
        put("worker.js", "import { T } from './threads.js';");
        put("pkg/rvp.js", "new URL('rvp_bg.wasm', import.meta.url);");
        put("pkg/rvp_bg.wasm", "\0asm");
        put("sw.js", "const V = 1;");
        let renamed = hash_assets(&dir).unwrap();
        let name = |f: &str| renamed.iter().find(|(a, _)| a == f).map(|(_, b)| b.clone()).unwrap();
        assert!(!renamed.iter().any(|(a, _)| a == "sw.js"));
        let html = std::fs::read_to_string(dir.join("index.html")).unwrap();
        assert!(html.contains(&name("style.css")) && html.contains(&name("main.js")), "{html}");
        let main = std::fs::read_to_string(dir.join(name("main.js"))).unwrap();
        assert!(main.contains(&name("threads.js")) && main.contains(&name("pkg/rvp.js")), "{main}");
        let glue = std::fs::read_to_string(dir.join(name("pkg/rvp.js"))).unwrap();
        assert!(glue.contains(name("pkg/rvp_bg.wasm").rsplit('/').next().unwrap()), "{glue}");
        let worker = std::fs::read_to_string(dir.join(name("worker.js"))).unwrap();
        assert!(worker.contains(&name("threads.js")), "{worker}");
        assert!(dir.join("sw.js").is_file() && !dir.join("main.js").exists());
        // The same input gives the same names.
        for (_, n) in &renamed {
            assert!(dir.join(n).is_file());
            let _ = std::fs::remove_file(dir.join(n));
        }
        for f in ["index.html", "sw.js"] {
            let _ = std::fs::remove_file(dir.join(f));
        }
    }
}
