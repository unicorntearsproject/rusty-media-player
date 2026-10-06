//! `cargo xtask dist publish`: copy the verified installers of this version, with a checksum file and signatures, to the local distribution
//! folder and to the S3 bucket. Versioned files are never overwritten: it stops before copying anything if any file of this version exists in
//! either place. Only the stable `latest` aliases (and the update manifest) are overwritten, and only last, after every versioned file is in
//! place and checked with `head-object`, so a half-failed publish never points `latest` at missing files.
//!
//! Published (names carry the version, as the files come out of the build): `rusty-wave_<ver>_amd64.deb`, `rusty-wave-<ver>-1.x86_64.rpm`,
//! `rusty-wave-<ver>-x86_64.AppImage` and its `.zsync`, a detached `.asc` for each, `rusty-wave-<ver>-SHA256SUMS` and its `.asc`. The Windows
//! installer and zip go along only with `--windows`, the macOS dmg only with `--macos`, the Flatpak bundle with `--flatpak`, the web app zip
//! with `--web`, the Linux tarball with `--tarball` (`--all` for the lot; say so only when they were built and verified in the same run). Aliases (`rusty-wave-latest-...`, byte copies with copies of the signatures) and `rusty-wave-latest.json` with its `.asc` follow.
//! The sums file is named by version because the destinations hold every version side by side.
use super::manifest::{self, Asset, Extras, MANIFEST_NAME, ZSYNC_ALIAS};
use super::*;

const LOCAL_DIR: &str = "/home/jj/projects/_software-dist/rusty-wave";
const BUCKET: &str = "ut-software-dist";
/// Versioned files never change; aliases and the manifest may be replaced by the next release, so caches must not keep them long.
const CACHE_VERSIONED: &str = "public, max-age=31536000, immutable";
const CACHE_ALIAS: &str = "public, max-age=300";

/// The files of a publish: versioned ones (never overwritten) and aliases `(source in the stage folder, alias name)` (overwritten).
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Plan {
    pub versioned: Vec<String>,
    pub aliases: Vec<(String, String)>,
}

/// The upload plan. Every file has a detached `.asc`; an alias carries a copy of its source's signature (a detached signature covers
/// the content, not the name). The manifest comes last among the aliases: it is what clients read.
pub(super) fn plan(ver: &str, assets: &[Asset]) -> Plan {
    let mut versioned = Vec::new();
    let mut aliases = Vec::new();
    for a in assets {
        versioned.push(a.name.clone());
        versioned.push(format!("{}.asc", a.name));
        aliases.push((a.name.clone(), a.alias.clone()));
        aliases.push((format!("{}.asc", a.name), format!("{}.asc", a.alias)));
        if let Some(z) = &a.zsync {
            versioned.push(z.clone());
            versioned.push(format!("{z}.asc"));
            aliases.push((z.clone(), ZSYNC_ALIAS.to_string()));
            aliases.push((format!("{z}.asc"), format!("{ZSYNC_ALIAS}.asc")));
        }
    }
    let sums = format!("rusty-wave-{ver}-SHA256SUMS");
    versioned.push(sums.clone());
    versioned.push(format!("{sums}.asc"));
    aliases.push((MANIFEST_NAME.to_string(), MANIFEST_NAME.to_string()));
    aliases.push((format!("{MANIFEST_NAME}.asc"), format!("{MANIFEST_NAME}.asc")));
    Plan { versioned, aliases }
}

fn content_type(name: &str) -> Option<&'static str> {
    if name.ends_with(".json") {
        Some("application/json")
    } else if name.ends_with(".asc") {
        Some("text/plain")
    } else if name.ends_with(".zsync") {
        Some("application/x-zsync")
    } else {
        None
    }
}

fn s3_cp(from: &Path, key: &str, cache: &str) -> Result<(), String> {
    let mut c = Command::new("aws");
    c.args(["s3", "cp", "--only-show-errors", "--cache-control", cache]);
    if let Some(t) = content_type(key) {
        c.args(["--content-type", t]);
    }
    sh(c.arg(from).arg(format!("s3://{BUCKET}/{key}")))
}

/// `aws s3api head-object`: the object's size, `None` when it does not exist, an error for anything else.
fn s3_size(key: &str) -> Result<Option<u64>, String> {
    let o = Command::new("aws")
        .args([
            "s3api",
            "head-object",
            "--bucket",
            BUCKET,
            "--key",
            key,
            "--query",
            "ContentLength",
            "--output",
            "text",
        ])
        .output()
        .map_err(|e| format!("aws: {e}"))?;
    if o.status.success() {
        let t = String::from_utf8_lossy(&o.stdout);
        return t
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| format!("head-object s3://{BUCKET}/{key}: unexpected `{}`", t.trim()));
    }
    let e = String::from_utf8_lossy(&o.stderr);
    if e.contains("404") || e.contains("Not Found") {
        Ok(None)
    } else {
        Err(format!("could not check s3://{BUCKET}/{key}: {}", e.trim()))
    }
}

impl Ctx {
    pub(super) fn publish(&self, x: Extras, dry_run: bool) -> Result<(), String> {
        if self.sign.is_none() {
            return Err("publish needs --sign (it signs the checksum file and the manifest and checks every signature first)".into());
        }
        if self.base_url.is_some() {
            return Err("publish always uses the bucket's URL: drop --base-url (it is for `dist manifest` and local tests)".into());
        }
        let ver = &self.version;
        if *ver != workspace_version(&self.root)? {
            return Err(format!(
                "publish: --version {ver} differs from Cargo.toml; the deb and rpm carry Cargo's version"
            ));
        }
        // 1. Everything in target/dist/release verifies against the committed public key (a throwaway keyring).
        self.verify()?;
        // 2. The files of this version.
        let assets = manifest::assets(ver, x);
        let plan = plan(ver, &assets);
        let out = self.out();
        let sums_name = format!("rusty-wave-{ver}-SHA256SUMS");
        for f in plan.versioned.iter().filter(|f| !f.starts_with(&sums_name)) {
            if !out.join(f).is_file() {
                return Err(format!(
                    "publish: {f} is missing in {} (build with --sign, then `dist checksums --sign`)",
                    out.display()
                ));
            }
        }
        // 3. A staging folder with exactly the published files, plus the checksum file for this version and the manifest, signed and checked.
        let stage = self.dist().join("publish").join(format!("rusty-wave-{ver}"));
        fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
        let mut sums = String::new();
        for f in plan.versioned.iter().filter(|f| !f.starts_with(&sums_name) && !f.ends_with(".asc")) {
            copy(&out.join(f), &stage.join(f))?;
            copy(&out.join(format!("{f}.asc")), &stage.join(format!("{f}.asc")))?;
            sums.push_str(&format!("{}  {f}\n", sha256(&stage.join(f))?));
        }
        write(&stage.join(&sums_name), sums.as_bytes())?;
        self.gpg_detach(&stage.join(&sums_name))?;
        sh(Command::new("sha256sum").current_dir(&stage).args(["-c", &sums_name]))?;
        // The manifest names the versioned files of this publish and their hashes; it is built from the staged copies.
        let manifest_path = self.write_manifest(&stage, &assets, true, &self.base_url())?;
        let fpr = sign::resolve_public_fpr(&self.root)?;
        for (data, sig) in [
            (stage.join(&sums_name), stage.join(format!("{sums_name}.asc"))),
            (manifest_path.clone(), stage.join(format!("{MANIFEST_NAME}.asc"))),
        ] {
            let o = Command::new("gpg")
                .env("GNUPGHOME", self.dist().join("verify-gnupg"))
                .args(["--batch", "--no-tty", "--status-fd", "1", "--verify"])
                .arg(&sig)
                .arg(&data)
                .output()
                .map_err(|e| e.to_string())?;
            if !String::from_utf8_lossy(&o.stdout).contains(&format!("VALIDSIG {fpr}")) {
                return Err(format!(
                    "publish: the signature {} does not verify against the public key",
                    sig.display()
                ));
            }
        }
        // The aliases are byte copies of staged files.
        for (src, alias) in plan.aliases.iter().filter(|(s, a)| s != a) {
            copy(&stage.join(src), &stage.join(alias))?;
        }
        // 4. Refuse to overwrite a versioned file: check both destinations before touching either.
        let local = Path::new(LOCAL_DIR);
        let clash: Vec<&String> = plan.versioned.iter().filter(|f| local.join(f).exists()).collect();
        if !clash.is_empty() {
            return Err(format!("publish: already in {LOCAL_DIR}: {clash:?}; bump the version"));
        }
        let mut clash = Vec::new();
        for f in &plan.versioned {
            if s3_size(f)?.is_some() {
                clash.push(f.clone());
            }
        }
        if !clash.is_empty() {
            return Err(format!("publish: already in s3://{BUCKET}/: {clash:?}; bump the version"));
        }
        println!(
            "publish: {} files and {} `latest` aliases, {} to {LOCAL_DIR} and s3://{BUCKET}/ (base URL {})",
            plan.versioned.len(),
            plan.aliases.len(),
            if dry_run { "would copy" } else { "copying" },
            self.base_url()
        );
        for f in &plan.versioned {
            println!("  {f}");
        }
        for (_, a) in &plan.aliases {
            println!("  {a}  (alias, overwritten)");
        }
        if dry_run {
            return Ok(());
        }
        // 5. Versioned files first, each checked on both sides, then the aliases (the manifest last).
        for f in &plan.versioned {
            copy(&stage.join(f), &local.join(f))?;
        }
        for f in &plan.versioned {
            s3_cp(&stage.join(f), f, CACHE_VERSIONED)?;
        }
        for f in &plan.versioned {
            let want = fs::metadata(stage.join(f)).map_err(|e| e.to_string())?.len();
            match s3_size(f)? {
                Some(got) if got == want => {}
                got => {
                    return Err(format!(
                        "publish: s3://{BUCKET}/{f} is {got:?} bytes after upload, expected {want}; the `latest` aliases were NOT updated"
                    ));
                }
            }
        }
        for (src, alias) in &plan.aliases {
            copy(&stage.join(src), &local.join(alias))?;
        }
        for (_, alias) in &plan.aliases {
            s3_cp(&stage.join(alias), alias, CACHE_ALIAS)?;
        }
        println!("published {} files and {} aliases for {ver}", plan.versioned.len(), plan.aliases.len());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_uploads_versioned_files_then_aliases() {
        let p = plan("0.0.3", &manifest::assets("0.0.3", Extras::ALL));
        // 9 files + zsync = 10 files, each with a signature, plus the sums and their signature.
        assert_eq!(p.versioned.len(), 10 * 2 + 2);
        assert!(p.versioned.iter().all(|f| f.contains("0.0.3")), "{:?}", p.versioned);
        assert!(p.versioned.contains(&"rusty-wave-0.0.3-x86_64.AppImage.zsync.asc".to_string()));
        // Aliases never carry a version and the manifest is the last upload.
        assert!(p.aliases.iter().all(|(_, a)| a.contains("latest")));
        assert_eq!(p.aliases.last().unwrap().1, "rusty-wave-latest.json.asc");
        assert_eq!(p.aliases[p.aliases.len() - 2].1, "rusty-wave-latest.json");
        // Versioned names never double as aliases (a publish may not overwrite them).
        assert!(p.aliases.iter().all(|(_, a)| !p.versioned.contains(a)));
        assert!(p.aliases.contains(&(
            "rusty-wave-0.0.3-x86_64.AppImage".into(),
            "rusty-wave-latest-x86_64.AppImage".into()
        )));
        assert!(p.aliases.contains(&(
            "rusty-wave-0.0.3-x86_64.AppImage.zsync".into(),
            "rusty-wave-latest-x86_64.AppImage.zsync".into()
        )));
        assert!(p.aliases.contains(&(
            "rusty-wave-0.0.3-macos-universal.dmg.asc".into(),
            "rusty-wave-latest-macos-universal.dmg.asc".into()
        )));
        assert!(p.aliases.contains(&(
            "io.github.idometeor.RustyWave-0.0.3.flatpak".into(),
            "io.github.idometeor.RustyWave-latest.flatpak".into()
        )));
        assert!(p.aliases.contains(&("rusty-wave-web-0.0.3.zip".into(), "rusty-wave-web-latest.zip".into())));
    }

    #[test]
    fn plan_without_optional_platforms() {
        let p = plan("0.0.3", &manifest::assets("0.0.3", Extras::default()));
        assert_eq!(p.versioned.len(), 4 * 2 + 2);
        assert!(!p.versioned.iter().any(|f| f.contains("Setup") || f.contains("dmg") || f.contains("zip")));
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type("rusty-wave-latest.json"), Some("application/json"));
        assert_eq!(content_type("x.AppImage.asc"), Some("text/plain"));
        assert_eq!(content_type("x.rpm"), None);
    }
}
