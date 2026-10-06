//! `cargo xtask dist macos`: `Rusty Wave.app` (a universal binary, arm64 + x86_64) in a `.dmg`.
//!
//! macOS only (needs `lipo`, `codesign`, `hdiutil`; the release workflow's `macos` job runs it). Signing hooks, both optional:
//! `RVP_MACOS_SIGN_IDENTITY` (a Developer ID Application identity: hardened runtime, timestamp, `packaging/macos/entitlements.plist`) and
//! `RVP_MACOS_NOTARY_PROFILE` (a `notarytool store-credentials` keychain profile: submit, wait, staple). Without an identity the bundle is
//! ad-hoc signed (`codesign --sign -`, which Apple Silicon requires to run at all) and the dmg is unsigned; Gatekeeper then asks the user to
//! allow it (see `docs/release-testing.md`). Nothing here has been run on macOS yet.
use super::*;

const TARGETS: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];
const APP_DIR: &str = "Rusty Wave.app";

/// `Info.plist` from the template: the version stamped in. macOS wants dotted integers in `CFBundleShortVersionString`, so a pre-release
/// suffix (`0.0.0-ci1`) is cut off there.
pub(super) fn info_plist(template: &str, version: &str) -> String {
    let short = version.split(['-', '+']).next().unwrap_or(version);
    template.replace("@SHORT_VERSION@", short).replace("@VERSION@", version)
}

/// The strings of every `<array>` that follows `<key>KEY</key>` in a plist (in order). A small scanner, not a plist parser: it only has to
/// read our own template, whose layout is fixed.
pub(super) fn plist_arrays(text: &str, key: &str) -> Vec<String> {
    let marker = format!("<key>{key}</key>");
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(&marker) {
        rest = &rest[i + marker.len()..];
        let trimmed = rest.trim_start();
        if let Some(body) = trimmed.strip_prefix("<array>") {
            let end = body.find("</array>").unwrap_or(body.len());
            let mut b = &body[..end];
            while let Some(s) = b.find("<string>") {
                b = &b[s + "<string>".len()..];
                let e = b.find("</string>").unwrap_or(b.len());
                out.push(b[..e].to_string());
                b = &b[e..];
            }
        }
    }
    out
}

/// The values of `<key>KEY</key><string>V</string>` pairs.
pub(super) fn plist_strings(text: &str, key: &str) -> Vec<String> {
    let marker = format!("<key>{key}</key>");
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(&marker) {
        rest = &rest[i + marker.len()..];
        if let Some(v) = rest.trim_start().strip_prefix("<string>") {
            if let Some(e) = v.find("</string>") {
                out.push(v[..e].to_string());
            }
        }
    }
    out
}

/// The extensions that `rusty-wave.iss` registers (`Software\Classes\.<ext>\OpenWithProgids`), sorted.
pub(super) fn iss_extensions(iss: &str) -> Vec<String> {
    let key = "Software\\Classes\\.";
    let mut v: Vec<String> = iss
        .lines()
        .filter_map(|l| {
            let i = l.find(key)? + key.len();
            let (ext, rest) = l[i..].split_once('\\')?;
            rest.starts_with("OpenWithProgids").then(|| ext.to_string())
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

/// What is wrong with the document types of an `Info.plist`, if anything: the extensions must be exactly `want`, every UTI must be a system
/// type or declared in `UTImportedTypeDeclarations`, and every declared type must be used.
pub(super) fn check_document_types(plist: &str, want: &[String]) -> Result<(), String> {
    let mut have = plist_arrays(plist, "CFBundleTypeExtensions");
    have.sort();
    have.dedup();
    let mut want = want.to_vec();
    want.sort();
    if have != want {
        let missing: Vec<_> = want.iter().filter(|e| !have.contains(e)).collect();
        let extra: Vec<_> = have.iter().filter(|e| !want.contains(e)).collect();
        return Err(format!(
            "Info.plist document types differ from rusty-wave.iss: missing {missing:?}, extra {extra:?}"
        ));
    }
    let used = plist_arrays(plist, "LSItemContentTypes");
    let declared = plist_strings(plist, "UTTypeIdentifier");
    let system =
        |u: &str| u.starts_with("public.") || u.starts_with("com.apple.") || u.starts_with("com.microsoft.");
    for u in &used {
        if !system(u) && !declared.contains(u) {
            return Err(format!(
                "Info.plist: `{u}` is neither a system type nor declared in UTImportedTypeDeclarations"
            ));
        }
    }
    for d in &declared {
        if !used.contains(d) {
            return Err(format!("Info.plist: `{d}` is declared but no document type uses it"));
        }
    }
    Ok(())
}

impl Ctx {
    fn macos_dir(&self) -> PathBuf {
        self.dist().join("macos")
    }

    pub(super) fn macos(&self) -> Result<(), String> {
        if !cfg!(target_os = "macos") {
            return Err("`dist macos` needs macOS (lipo, codesign, hdiutil); the release workflow's `macos` job runs it on a macos-14 runner".into());
        }
        let notary = std::env::var("RVP_MACOS_NOTARY_PROFILE").ok().filter(|s| !s.is_empty());
        let identity = std::env::var("RVP_MACOS_SIGN_IDENTITY").ok().filter(|s| !s.is_empty());
        if notary.is_some() && identity.is_none() {
            return Err("RVP_MACOS_NOTARY_PROFILE needs RVP_MACOS_SIGN_IDENTITY (only a Developer ID signed app can be notarized)".into());
        }
        let dir = self.macos_dir();
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // 1. The universal binary.
        let universal = dir.join("rusty-wave");
        if !(self.no_build && universal.exists()) {
            let mut slices = Vec::new();
            for t in TARGETS {
                sh(self.cargo_cmd().args([
                    "build",
                    "--profile",
                    "dist",
                    "-p",
                    "rvp-host-desktop",
                    "--target",
                    t,
                    "--locked",
                ]))?;
                slices.push(self.root.join("target").join(t).join("dist/rusty-wave"));
            }
            let mut c = Command::new("lipo");
            c.arg("-create").arg("-output").arg(&universal);
            for s in &slices {
                c.arg(s);
            }
            sh(&mut c)?;
        }
        println!("{}", capture(Command::new("lipo").arg("-archs").arg(&universal))?.trim());
        // 2. The bundle (overwritten in place).
        let app = dir.join(APP_DIR);
        let macos_bin = app.join("Contents/MacOS/rusty-wave");
        copy(&universal, &macos_bin)?;
        set_mode(&macos_bin, 0o755)?;
        let template =
            fs::read_to_string(self.root.join("packaging/macos/Info.plist.in")).map_err(|e| e.to_string())?;
        write(&app.join("Contents/Info.plist"), info_plist(&template, &self.version).as_bytes())?;
        write(&app.join("Contents/PkgInfo"), b"APPL????")?;
        let res = app.join("Contents/Resources");
        copy(&self.root.join("packaging/icons/rusty-wave.icns"), &res.join("rusty-wave.icns"))?;
        for f in ["LICENSE-MIT", "LICENSE-APACHE", "THIRD_PARTY_LICENSES.md"] {
            copy(&self.root.join(f), &res.join(f))?;
        }
        sh(Command::new("plutil").arg("-lint").arg(app.join("Contents/Info.plist")))?;
        // 3. Sign: Developer ID with the hardened runtime, or ad hoc (needed on Apple Silicon).
        let ent = self.root.join("packaging/macos/entitlements.plist");
        match &identity {
            Some(id) => {
                sh(Command::new("codesign")
                    .args(["--force", "--options", "runtime", "--timestamp", "--entitlements"])
                    .arg(&ent)
                    .args(["--sign", id])
                    .arg(&app))?;
                sh(Command::new("codesign").args(["--verify", "--strict", "--verbose=2"]).arg(&app))?;
            }
            None => {
                println!(
                    "RVP_MACOS_SIGN_IDENTITY is not set: ad-hoc signing only. The app is NOT signed with a Developer ID and will not be notarized; Gatekeeper will ask the user to allow it."
                );
                sh(Command::new("codesign").args(["--force", "--sign", "-"]).arg(&app))?;
            }
        }
        // 4. Notarize and staple the app (so it also passes Gatekeeper when copied out of the dmg).
        if let Some(profile) = &notary {
            let zip = dir.join("Rusty Wave-notarize.zip");
            sh(Command::new("ditto").args(["-c", "-k", "--keepParent"]).arg(&app).arg(&zip))?;
            self.notarize(&zip, profile)?;
            sh(Command::new("xcrun").arg("stapler").arg("staple").arg(&app))?;
        }
        // 5. The dmg: the app and a link to /Applications.
        let stage = dir.join("dmg");
        fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
        sh(Command::new("ditto").arg(&app).arg(stage.join(APP_DIR)))?;
        let link = stage.join("Applications");
        if link.symlink_metadata().is_err() {
            sh(Command::new("ln").args(["-s", "/Applications"]).arg(&link))?;
        }
        let dmg = dir.join(format!("rusty-wave-{}-macos-universal.dmg", self.version));
        if dmg.exists() {
            fs::remove_file(&dmg).map_err(|e| e.to_string())?;
        }
        if have("create-dmg") {
            // `create-dmg` lays out the window (app on the left, the link on the right) and makes the volume icon.
            sh(Command::new("create-dmg")
                .args(["--volname", "Rusty Wave", "--window-size", "640", "380", "--icon-size", "96"])
                .args([
                    "--icon",
                    APP_DIR,
                    "170",
                    "180",
                    "--icon",
                    "Applications",
                    "470",
                    "180",
                    "--hide-extension",
                    APP_DIR,
                ])
                .arg(&dmg)
                .arg(&stage))?;
        } else {
            sh(Command::new("hdiutil")
                .args(["create", "-volname", "Rusty Wave", "-format", "UDZO", "-ov", "-srcfolder"])
                .arg(&stage)
                .arg(&dmg))?;
        }
        if let Some(id) = &identity {
            sh(Command::new("codesign").args(["--force", "--timestamp", "--sign", id]).arg(&dmg))?;
        }
        if let Some(profile) = &notary {
            self.notarize(&dmg, profile)?;
            sh(Command::new("xcrun").arg("stapler").arg("staple").arg(&dmg))?;
        }
        let out = self.out().join(dmg.file_name().unwrap());
        copy(&dmg, &out)?;
        println!(
            "{} ({})",
            out.display(),
            match (&identity, &notary) {
                (Some(_), Some(_)) => "Developer ID signed, notarized and stapled",
                (Some(_), None) => "Developer ID signed, not notarized",
                _ => "unsigned (ad-hoc): Gatekeeper will ask the user to allow it",
            }
        );
        Ok(())
    }

    /// `xcrun notarytool submit <file> --keychain-profile <p> --wait`; a rejection is an error.
    fn notarize(&self, file: &Path, profile: &str) -> Result<(), String> {
        let out = capture(Command::new("xcrun").args(["notarytool", "submit"]).arg(file).args([
            "--keychain-profile",
            profile,
            "--wait",
        ]))?;
        println!("{out}");
        if out.contains("status: Accepted") {
            Ok(())
        } else {
            Err(format!("notarization of {} was not accepted:\n{out}", file.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template() -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../packaging/macos/Info.plist.in");
        fs::read_to_string(p).unwrap()
    }

    fn iss() -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../packaging/windows/rusty-wave.iss");
        fs::read_to_string(p).unwrap()
    }

    #[test]
    fn version_is_stamped_and_prereleases_get_a_dotted_short_version() {
        let t = "<string>@VERSION@</string><string>@SHORT_VERSION@</string>";
        assert_eq!(info_plist(t, "0.0.3"), "<string>0.0.3</string><string>0.0.3</string>");
        assert_eq!(info_plist(t, "0.0.0-ci1"), "<string>0.0.0-ci1</string><string>0.0.0</string>");
        assert!(!info_plist(&template(), "1.2.3").contains('@'));
    }

    #[test]
    fn the_shipped_plist_agrees_with_the_installer() {
        let want = iss_extensions(&iss());
        assert_eq!(want.len(), 17, "{want:?}");
        check_document_types(&template(), &want).unwrap();
        let p = template();
        assert_eq!(plist_strings(&p, "CFBundleIdentifier"), ["io.github.idometeor.RustyWave"]);
        assert_eq!(plist_strings(&p, "LSMinimumSystemVersion"), ["11.0"]);
        assert_eq!(plist_strings(&p, "CFBundleExecutable"), ["rusty-wave"]);
    }

    #[test]
    fn document_type_mistakes_are_found() {
        let want = iss_extensions(&iss());
        let p = template();
        // An extension the installer does not have.
        let e = check_document_types(&p.replace("<string>pls</string>", "<string>xyz</string>"), &want)
            .unwrap_err();
        assert!(e.contains("missing") && e.contains("pls") && e.contains("xyz"), "{e}");
        // A UTI that is neither a system type nor declared.
        let e = check_document_types(
            &p.replacen("<string>org.xiph.flac</string>", "<string>org.example.flac</string>", 1),
            &want,
        )
        .unwrap_err();
        assert!(e.contains("org.example.flac") || e.contains("org.xiph.flac"), "{e}");
        // A declaration nothing uses.
        let e = check_document_types(
            &p.replacen("<string>org.xiph.opus</string>", "<string>public.audio</string>", 1),
            &want,
        )
        .unwrap_err();
        assert!(e.contains("org.xiph.opus"), "{e}");
    }
}
