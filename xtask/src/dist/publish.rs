//! `cargo xtask dist publish`: lay the signed, verified release out for Rusty Bucket's release site (software.rustybucket.ai) and check it against
//! their input contract; nothing is uploaded (their publisher does that: the CI `publish` job's pinned action, or
//! `../rusty-bucket-aws/infra/scripts/publish-release.sh` by hand). `<out>/<version>/` holds the versioned files, `<out>/latest/` the aliases
//! and the signed manifest, `<out>/release/` exactly the directory the publisher takes (see `rbcheck.rs`).
//!
//! Published (names carry the version, as the files come out of the build): `rusty-wave_<deb version>_amd64.deb`,
//! `rusty-wave-<ver>-<release>.x86_64.rpm`, `rusty-wave-<ver>-x86_64.AppImage` and its `.zsync`, a detached `.asc` for each,
//! `rusty-wave-<ver>-SHA256SUMS` and its `.asc`. The Windows installer and zip go along only with `--windows`, the macOS dmg only with `--macos`,
//! the Flatpak bundle with `--flatpak`, the web app zip with `--web`, the Linux tarball with `--tarball` (`--all` for the lot; say so only when
//! they were built and verified in the same run). Aliases (`rusty-wave-latest...`, byte copies with copies of the signatures) and
//! `rusty-wave-latest.json` with its `.asc` are in `latest/`. The sums file is named by version because the destination holds every version.
use super::manifest::{self, Asset, Extras, MANIFEST_NAME, ZSYNC_ALIAS};
use super::*;

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

impl Ctx {
    /// Lay the signed, verified release out for Rusty Bucket's release site and stop there (nothing is uploaded: their publisher does that). `<out>/<version>/` holds the versioned
    /// files, `<out>/latest/` the aliases and the signed manifest, whose URLs name `<site>/<version>/<file>`; the AppImage's update
    /// information (embedded when it was built with the same `--target`) names `<site>/latest/`.
    fn stage_rustybucket(&self, stage: &Path, plan: &Plan, dry_run: bool) -> Result<(), String> {
        let ver = &self.version;
        let site = self.dist().join("publish-rb").join("rusty-wave");
        println!(
            "publish (rustybucket): {} files in {ver}/ and {} in latest/, site {} ({})",
            plan.versioned.len(),
            plan.aliases.len(),
            manifest::RB_BASE_URL,
            if dry_run { "dry run: nothing written" } else { "staged, not uploaded" }
        );
        if dry_run {
            return Ok(());
        }
        for f in &plan.versioned {
            copy(&stage.join(f), &site.join(ver).join(f))?;
        }
        for (_, alias) in &plan.aliases {
            copy(&stage.join(alias), &site.join("latest").join(alias))?;
        }
        // The directory their publisher takes: the versioned files, the signed manifest, and nothing else.
        let rel = self.rb_release_dir();
        for f in
            plan.versioned.iter().chain([MANIFEST_NAME.to_string(), format!("{MANIFEST_NAME}.asc")].iter())
        {
            copy(&stage.join(f), &rel.join(f))?;
        }
        self.verify_rustybucket()?;
        println!("{}", site.display());
        println!(
            "upload with ../rusty-bucket-aws/infra/scripts/publish-release.sh {} (Rusty Bucket's script; not run by this command)",
            rel.display()
        );
        Ok(())
    }

    pub(super) fn publish(&self, x: Extras, dry_run: bool) -> Result<(), String> {
        if self.sign.is_none() {
            return Err("publish needs --sign (it signs the checksum file and the manifest and checks every signature first)".into());
        }
        if self.base_url.is_some() {
            return Err("publish always uses the release site's URL: drop --base-url (it is for `dist manifest` and local tests)".into());
        }
        let ver = &self.version;
        if *ver != workspace_version(&self.root)? {
            return Err(format!(
                "publish: --version {ver} differs from Cargo.toml; the deb and rpm carry Cargo's version"
            ));
        }
        // 1. Everything in target/dist/release verifies against the committed public key (a throwaway keyring).
        // Only the detached signatures are required: CI signs after building, so the rpm and the AppImage carry no embedded one.
        self.verify_with(false)?;
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
        let manifest_path = self.write_manifest(&stage, &assets, true)?;
        let fpr = sign::resolve_public_fpr(&self.root)?;
        let guard = sign::GpgHome::new(self.dist().join("verify-gnupg"))?;
        for (data, sig) in [
            (stage.join(&sums_name), stage.join(format!("{sums_name}.asc"))),
            (manifest_path.clone(), stage.join(format!("{MANIFEST_NAME}.asc"))),
        ] {
            let o = Command::new("gpg")
                .env("GNUPGHOME", guard.path())
                .args(["--batch", "--no-tty", "--status-fd", "1", "--verify"])
                .arg(&sig)
                .arg(&data)
                .output()
                .map_err(|e| e.to_string())?;
            if !super::rbcheck::valid_sig_by(&String::from_utf8_lossy(&o.stdout), &fpr) {
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
        self.stage_rustybucket(&stage, &plan, dry_run)
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
            "io.github.unicorntearsproject.RustyWave-0.0.3.flatpak".into(),
            "io.github.unicorntearsproject.RustyWave-latest.flatpak".into()
        )));
        assert!(p.aliases.contains(&("rusty-wave-web-0.0.3.zip".into(), "rusty-wave-web-latest.zip".into())));
    }

    #[test]
    fn plan_without_optional_platforms() {
        let p = plan("0.0.3", &manifest::assets("0.0.3", Extras::default()));
        assert_eq!(p.versioned.len(), 4 * 2 + 2);
        assert!(!p.versioned.iter().any(|f| f.contains("Setup") || f.contains("dmg") || f.contains("zip")));
    }
}
