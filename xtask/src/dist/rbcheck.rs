//! `dist verify --target rustybucket`: the release directory Rusty Bucket's publisher accepts, checked offline against their input contract
//! (`../rusty-bucket-aws/infra/docs/operations/publish-release.md`, "What the release directory must contain"). `publish --target rustybucket`
//! writes it to `target/dist/publish-rb/rusty-wave/release` and runs this check on it; the publisher's own script repeats every check before it
//! uploads anything, so a failure here is a failure there, found sooner.
//!
//! The contract: only the files on the product's list (names as in `manifest::assets`, plus the `.zsync`, the sums file and the manifest),
//! a detached `.asc` next to every one of them by the release key, `rusty-wave-<v>-SHA256SUMS` with bare file names listing every release
//! file, the signed manifest with URLs under `<site>/<v>/`, and the AppImage with its `.zsync` (whose `URL:` names the versioned AppImage).
use super::manifest::{self, Extras, MANIFEST_NAME, RB_BASE_URL, ZSYNC_ALIAS};
use super::*;

/// What is wrong with gpg's `--status-fd` output for a signature that must be by the release key `fpr`, or `Ok`: exactly one signature, one
/// `GOODSIG` and one `VALIDSIG`, none of the refusals gpg reports (`BADSIG`, `ERRSIG`, `EXPSIG`, `EXPKEYSIG`, `REVKEYSIG`, `NO_PUBKEY`,
/// `FAILURE`), and the primary key's fingerprint as the last field of the `VALIDSIG` line (the first field is the signing subkey's own).
pub(super) fn signature_problem(status: &str, fpr: &str) -> Result<(), String> {
    const REFUSED: [&str; 7] =
        ["BADSIG", "ERRSIG", "EXPSIG", "EXPKEYSIG", "REVKEYSIG", "NO_PUBKEY", "FAILURE"];
    let word =
        |l: &&str| l.strip_prefix("[GNUPG:] ").and_then(|r| r.split_whitespace().next()).map(str::to_string);
    let lines: Vec<&str> = status.lines().collect();
    for l in &lines {
        if let Some(w) = word(l) {
            if REFUSED.contains(&w.as_str()) {
                return Err(format!("gpg says {w}"));
            }
        }
    }
    let count = |w: &str| lines.iter().filter(|l| word(l).as_deref() == Some(w)).count();
    if count("GOODSIG") != 1 || count("VALIDSIG") != 1 {
        return Err(format!(
            "{} GOODSIG and {} VALIDSIG lines, exactly one signature is required",
            count("GOODSIG"),
            count("VALIDSIG")
        ));
    }
    let valid = lines.iter().find(|l| word(l).as_deref() == Some("VALIDSIG")).unwrap();
    if valid.split_whitespace().last() != Some(fpr) {
        return Err(format!("not made by {fpr} or one of its subkeys"));
    }
    Ok(())
}

/// Whether the signature is by the release key `fpr` (see `signature_problem`).
pub(super) fn valid_sig_by(status: &str, fpr: &str) -> bool {
    signature_problem(status, fpr).is_ok()
}

/// The problems a release directory has against the contract, offline and without gpg (names, sums, manifest, zsync). Empty when it is fine.
pub(super) fn check_files(dir: &Path, version: &str) -> Result<Vec<String>, String> {
    let assets = manifest::assets(version, Extras::ALL);
    let mut allowed: Vec<String> = Vec::new();
    for a in &assets {
        allowed.push(a.name.clone());
        allowed.extend(a.zsync.clone());
    }
    let sums_name = format!("rusty-wave-{version}-SHA256SUMS");
    allowed.push(sums_name.clone());
    allowed.push(MANIFEST_NAME.to_string());
    let mut present = Vec::new();
    for e in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let e = e.map_err(|e| e.to_string())?;
        let name = e.file_name().to_string_lossy().into_owned();
        let ft = e.file_type().map_err(|e| e.to_string())?;
        if !ft.is_file() {
            return Ok(vec![format!("{name}: not a regular file (no subdirectories or links)")]);
        }
        present.push(name);
    }
    present.sort();
    let mut bad = Vec::new();
    for n in &present {
        let base = n.strip_suffix(".asc").unwrap_or(n);
        if !allowed.iter().any(|a| a == base) {
            bad.push(format!("{n}: not on the rusty-wave {version} file list"));
        }
        if !n.ends_with(".asc") && !present.contains(&format!("{n}.asc")) {
            bad.push(format!("{n}: missing {n}.asc"));
        }
    }
    for need in [&sums_name, &MANIFEST_NAME.to_string()] {
        if !present.contains(need) {
            bad.push(format!("{need}: missing"));
        }
    }
    let appimage = format!("rusty-wave-{version}-x86_64.AppImage");
    let zsync = format!("{appimage}.zsync");
    if present.contains(&appimage) != present.contains(&zsync) {
        bad.push(format!("{appimage} and {zsync} must be published together"));
    }
    // The sums: bare names, every release file listed, every hash right.
    if let Ok(text) = fs::read_to_string(dir.join(&sums_name)) {
        let mut listed = Vec::new();
        for line in text.lines().filter(|l| !l.is_empty()) {
            let Some((hash, name)) = line.split_once("  ") else {
                bad.push(format!("{sums_name}: bad line `{line}`"));
                continue;
            };
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
                bad.push(format!("{sums_name}: bad hash in `{line}`"));
            }
            if name.starts_with("./") || name.contains('/') {
                bad.push(format!("{sums_name}: `{name}` is not a bare file name"));
            }
            match sha256(&dir.join(name)) {
                Ok(h) if h == hash => {}
                Ok(_) => bad.push(format!("{name}: SHA-256 differs from {sums_name}")),
                Err(_) => bad.push(format!("{sums_name} lists {name}, which is missing")),
            }
            listed.push(name.to_string());
        }
        for n in &present {
            if !n.ends_with(".asc") && *n != sums_name && n != MANIFEST_NAME && !listed.contains(n) {
                bad.push(format!("{n}: not listed in {sums_name}"));
            }
        }
    }
    bad.extend(check_manifest(dir, version, &assets));
    // The zsync control file downloads the versioned AppImage, by length.
    if let Ok(z) = fs::read(dir.join(&zsync)) {
        let header: String = String::from_utf8_lossy(&z).split("\n\n").next().unwrap_or("").to_string();
        let want_url = format!("{RB_BASE_URL}/{version}/{appimage}");
        let get =
            |k: &str| header.lines().find_map(|l| l.strip_prefix(k)?.strip_prefix(": ")).map(str::to_string);
        if get("URL").as_deref() != Some(want_url.as_str()) {
            bad.push(format!("{zsync}: URL is {:?}, expected {want_url:?}", get("URL")));
        }
        if let Ok(m) = fs::metadata(dir.join(&appimage)) {
            if get("Length") != Some(m.len().to_string()) {
                bad.push(format!("{zsync}: Length {:?} != {}", get("Length"), m.len()));
            }
        }
    }
    Ok(bad)
}

/// The manifest against the files: version, key, and for every file its name, URL, signature URL, size and hash; nothing points elsewhere.
fn check_manifest(dir: &Path, version: &str, assets: &[manifest::Asset]) -> Vec<String> {
    let Ok(text) = fs::read_to_string(dir.join(MANIFEST_NAME)) else { return vec![] };
    let mut bad = Vec::new();
    let vdir = format!("{RB_BASE_URL}/{version}");
    for (what, want) in
        [("schema", "\"schema\": 1,".to_string()), ("version", format!("\"version\": \"{version}\","))]
    {
        if !text.contains(&want) {
            bad.push(format!("{MANIFEST_NAME}: {what} is not {want}"));
        }
    }
    if !text.contains("\"key_fingerprint\": \"E13FF843723D54068E45A3FF54BF2FA407093CEE\"") {
        bad.push(format!("{MANIFEST_NAME}: key_fingerprint is not the release key's"));
    }
    for a in assets {
        let Ok(meta) = fs::metadata(dir.join(&a.name)) else {
            bad.push(format!("{MANIFEST_NAME}: {} is not in the release directory", a.name));
            continue;
        };
        let sha = sha256(&dir.join(&a.name)).unwrap_or_default();
        for want in [
            format!("\"name\": \"{}\"", a.name),
            format!("\"url\": \"{vdir}/{}\"", a.name),
            format!("\"signature_url\": \"{vdir}/{}.asc\"", a.name),
            format!("\"size\": {}", meta.len()),
            format!("\"sha256\": \"{sha}\""),
        ] {
            if !text.contains(&want) {
                bad.push(format!("{MANIFEST_NAME}: {} has no {want}", a.name));
            }
        }
        if a.zsync.is_some() {
            let want = format!("\"zsync_url\": \"{RB_BASE_URL}/latest/{ZSYNC_ALIAS}\"");
            if !text.contains(&want) {
                bad.push(format!("{MANIFEST_NAME}: the AppImage has no {want}"));
            }
        }
    }
    for l in text.lines() {
        let l = l.trim();
        if (l.starts_with("\"url\"") || l.starts_with("\"signature_url\"") || l.starts_with("\"zsync_url\""))
            && !l.contains(&format!("\"{RB_BASE_URL}/"))
        {
            bad.push(format!("{MANIFEST_NAME}: {l} is not under {RB_BASE_URL}/"));
        }
    }
    bad
}

impl Ctx {
    /// Where `publish --target rustybucket` leaves the directory the publisher takes.
    pub(super) fn rb_release_dir(&self) -> PathBuf {
        self.dist().join("publish-rb").join("rusty-wave").join("release")
    }

    /// `dist verify --target rustybucket`: the files of the release directory against the contract, then every `.asc` against the public key in
    /// the repository (a throwaway keyring), made by the release key or its signing subkey.
    pub(super) fn verify_rustybucket(&self) -> Result<(), String> {
        let dir = self.rb_release_dir();
        if !dir.is_dir() {
            return Err(format!(
                "{} does not exist: run `cargo xtask dist publish --target rustybucket --all --sign` first",
                dir.display()
            ));
        }
        let mut bad = check_files(&dir, &self.version)?;
        let fpr = sign::resolve_public_fpr(&self.root)?;
        let home = self.dist().join("verify-gnupg");
        fs::create_dir_all(&home).map_err(|e| e.to_string())?;
        set_mode(&home, 0o700)?;
        sh(Command::new("gpg")
            .env("GNUPGHOME", &home)
            .args(["--batch", "--no-tty", "--quiet", "--import"])
            .arg(self.root.join(sign::KEY_ASC)))?;
        let mut names: Vec<String> = fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .filter_map(|e| Some(e.ok()?.file_name().to_string_lossy().into_owned()))
            .filter(|n| n.ends_with(".asc"))
            .collect();
        names.sort();
        for sig in &names {
            let o = Command::new("gpg")
                .env("GNUPGHOME", &home)
                .args(["--batch", "--no-tty", "--status-fd", "1", "--verify"])
                .arg(dir.join(sig))
                .arg(dir.join(sig.trim_end_matches(".asc")))
                .output()
                .map_err(|e| e.to_string())?;
            let status = String::from_utf8_lossy(&o.stdout);
            if !o.status.success() {
                bad.push(format!("{sig}: gpg could not verify it"));
            } else if let Err(why) = signature_problem(&status, &fpr) {
                bad.push(format!("{sig}: {why}"));
            }
        }
        if bad.is_empty() {
            println!(
                "rustybucket release dir ok: {} ({} files, {} signatures)",
                dir.display(),
                names.len() * 2,
                names.len()
            );
            Ok(())
        } else {
            Err(format!("the release directory does not meet the contract:\n  {}", bad.join("\n  ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIMARY: &str = "E13FF843723D54068E45A3FF54BF2FA407093CEE";
    const SUB: &str = "2FD1848657B706A3576877E071DB2DB049A19B84";

    fn good(by: &str) -> String {
        format!(
            "[GNUPG:] NEWSIG\n[GNUPG:] GOODSIG 71DB2DB049A19B84 x\n[GNUPG:] VALIDSIG {by} 2026-10-07 1 0 4 0 22 10 00 {PRIMARY}\n"
        )
    }

    #[test]
    fn a_signature_by_the_primary_or_by_its_subkey_is_accepted_and_nothing_else_is() {
        assert!(valid_sig_by(&good(SUB), PRIMARY));
        assert!(valid_sig_by(&good(PRIMARY), PRIMARY));
        // Another key's signature, including one whose subkey merely shares the fingerprint position.
        assert!(!valid_sig_by(&good(SUB), "0000000000000000000000000000000000000000"));
        assert!(!valid_sig_by("[GNUPG:] BADSIG 1 x\n", PRIMARY));
        // Two signatures in one file, or any of gpg's refusals next to a good one.
        let two = format!("{}{}", good(SUB), good(PRIMARY));
        assert!(signature_problem(&two, PRIMARY).unwrap_err().contains("exactly one"));
        for bad in ["BADSIG", "ERRSIG", "EXPSIG", "EXPKEYSIG", "REVKEYSIG", "NO_PUBKEY", "FAILURE"] {
            let s = format!("{}[GNUPG:] {bad} 1 x\n", good(SUB));
            assert!(signature_problem(&s, PRIMARY).unwrap_err().contains(bad), "{bad}");
        }
        // The primary must be the LAST field: a line that only starts with it (the primary signing itself is fine, a foreign subkey is not).
        let foreign =
            format!("[GNUPG:] GOODSIG 1 x\n[GNUPG:] VALIDSIG {PRIMARY} 2026-10-07 1 0 4 0 22 10 00 {SUB}\n");
        assert!(signature_problem(&foreign, PRIMARY).is_err());
    }

    /// A complete, fake release directory for `v` (contents are junk; the names, sums, manifest and zsync header are what the check reads).
    fn fake_release(tag: &str, v: &str) -> PathBuf {
        let base = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let dir = base.join("rbcheck-tests").join(tag);
        fs::create_dir_all(&dir).unwrap();
        for e in fs::read_dir(&dir).unwrap() {
            fs::remove_file(e.unwrap().path()).unwrap();
        }
        let assets = manifest::assets(v, Extras::ALL);
        let mut entries = Vec::new();
        let mut sums = String::new();
        let mut files: Vec<String> = Vec::new();
        for a in &assets {
            let body = format!("body of {}", a.name);
            fs::write(dir.join(&a.name), &body).unwrap();
            files.push(a.name.clone());
            if let Some(z) = &a.zsync {
                let header = format!(
                    "zsync: 0.6.2\nFilename: {}\nURL: {RB_BASE_URL}/{v}/{}\nLength: {}\n\nbinary",
                    a.name,
                    a.name,
                    body.len()
                );
                fs::write(dir.join(z), header).unwrap();
                files.push(z.clone());
            }
            entries.push(manifest::Entry {
                key: a.key,
                arch: a.arch,
                name: a.name.clone(),
                size: body.len() as u64,
                sha256: sha256(&dir.join(&a.name)).unwrap(),
                zsync: a.zsync.is_some(),
                note: None,
            });
        }
        for f in &files {
            sums.push_str(&format!("{}  {f}\n", sha256(&dir.join(f)).unwrap()));
        }
        let sums_name = format!("rusty-wave-{v}-SHA256SUMS");
        fs::write(dir.join(&sums_name), sums).unwrap();
        let text = manifest::render(
            v,
            "2026-10-07",
            "E13FF843723D54068E45A3FF54BF2FA407093CEE",
            &format!("{RB_BASE_URL}/{v}"),
            &format!("{RB_BASE_URL}/latest"),
            &entries,
        );
        fs::write(dir.join(MANIFEST_NAME), text).unwrap();
        for f in files.iter().chain([&sums_name, &MANIFEST_NAME.to_string()]) {
            fs::write(dir.join(format!("{f}.asc")), "sig").unwrap();
        }
        dir
    }

    #[test]
    fn a_complete_release_directory_meets_the_contract() {
        let dir = fake_release("ok", "1.0.0-rc2");
        assert_eq!(check_files(&dir, "1.0.0-rc2").unwrap(), Vec::<String>::new());
        // The packaging systems' own spellings of a pre-release.
        assert!(dir.join("rusty-wave_1.0.0~rc2_amd64.deb").is_file());
        assert!(dir.join("rusty-wave-1.0.0-0.1.rc2.x86_64.rpm").is_file());
    }

    #[test]
    fn what_the_contract_forbids_is_reported() {
        let dir = fake_release("bad", "1.0.0-rc2");
        let v = "1.0.0-rc2";
        // A stray file, a missing signature, a "./" in the sums, an alias file and a wrong zsync URL.
        fs::write(dir.join("index.html"), "x").unwrap();
        fs::remove_file(dir.join("rusty-wave-web-1.0.0-rc2.zip.asc")).unwrap();
        fs::write(dir.join("rusty-wave-latest-x86_64.AppImage"), "x").unwrap();
        let sums = dir.join(format!("rusty-wave-{v}-SHA256SUMS"));
        let t = fs::read_to_string(&sums).unwrap().replacen("  rusty-wave", "  ./rusty-wave", 1);
        fs::write(&sums, t).unwrap();
        let z = dir.join("rusty-wave-1.0.0-rc2-x86_64.AppImage.zsync");
        let t = fs::read_to_string(&z).unwrap().replace("software.rustybucket.ai", "example.com");
        fs::write(&z, t).unwrap();
        let bad = check_files(&dir, v).unwrap().join("\n");
        for want in [
            "index.html: not on the rusty-wave 1.0.0-rc2 file list",
            "rusty-wave-web-1.0.0-rc2.zip: missing rusty-wave-web-1.0.0-rc2.zip.asc",
            "rusty-wave-latest-x86_64.AppImage: not on the",
            "is not a bare file name",
            "URL is",
        ] {
            assert!(bad.contains(want), "{want} not in:\n{bad}");
        }
    }
}
