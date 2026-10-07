//! GPG signing and signature checks for `cargo xtask dist --sign` / `dist verify`.
//!
//! The release key ("Rusty Wave Release", ed25519, sign-only) lives in the maintainer's own gpg keyring; only its public half is in the
//! repository (`packaging/keys/rusty-wave-release.asc` and the binary `.gpg`). Nothing here reads or writes a secret key. What gets signed:
//! rpm (rpmsign, embedded), AppImage (appimagetool, embedded), the apt repo (`InRelease` and `Release.gpg`), the Flatpak repo (commit, summary),
//! and a detached `.asc` for every artifact and `SHA256SUMS`. `verify` checks all of it against the public key in a throwaway keyring.
use super::*;

pub(super) const KEY_ASC: &str = "packaging/keys/rusty-wave-release.asc";
const KEY_GPG: &str = "packaging/keys/rusty-wave-release.gpg";

/// The full fingerprint to sign with: `explicit`, else `RVP_GPG_KEY`, else the key in `packaging/keys/rusty-wave-release.asc`. Its secret half must be in
/// the keyring (a missing key is an error here, not in the middle of a long build).
pub(super) fn resolve_key(root: &Path, explicit: Option<String>) -> Result<String, String> {
    let wanted = match explicit.or_else(|| std::env::var("RVP_GPG_KEY").ok().filter(|k| !k.is_empty())) {
        Some(k) => k.replace(' ', ""),
        None => fingerprint(&capture(
            Command::new("gpg").args(["--batch", "--show-keys", "--with-colons"]).arg(root.join(KEY_ASC)),
        )?)
        .ok_or_else(|| format!("no key in {KEY_ASC}"))?,
    };
    let secret =
        capture(Command::new("gpg").args(["--batch", "--list-secret-keys", "--with-colons", &wanted]))
            .map_err(|_| {
                format!("the secret key {wanted} is not in your gpg keyring (see docs/packaging.md, Signing)")
            })?;
    fingerprint(&secret).ok_or_else(|| format!("no secret key {wanted}"))
}

/// The first `fpr` record of `gpg --with-colons` output.
fn fingerprint(colons: &str) -> Option<String> {
    colons.lines().find(|l| l.starts_with("fpr:")).and_then(|l| l.split(':').nth(9)).map(str::to_string)
}

impl Ctx {
    fn key(&self) -> Result<&str, String> {
        self.sign.as_deref().ok_or_else(|| "this needs --sign (the release key)".to_string())
    }

    /// `<file>.asc`, an armoured detached signature.
    pub(super) fn gpg_detach(&self, file: &Path) -> Result<(), String> {
        let mut asc = file.as_os_str().to_owned();
        asc.push(".asc");
        sh(Command::new("gpg")
            .args(["--batch", "--yes", "--armor", "--detach-sign", "--local-user", self.key()?, "--output"])
            .arg(&asc)
            .arg(file))
    }

    pub(super) fn sign_deb(&self, deb: &Path) -> Result<(), String> {
        // apt trusts the signed Release file (see `apt_repo`), not signatures inside the .deb (dpkg-sig), so a detached signature it is.
        self.gpg_detach(deb)
    }

    /// An embedded rpm signature (`rpm -K` checks it once the public key is imported), or a detached one where rpmsign is missing.
    pub(super) fn sign_rpm(&self, rpm: &Path) -> Result<(), String> {
        let Some(key) = &self.sign else { return Ok(()) };
        if have("rpmsign") {
            sh(Command::new("rpmsign")
                .arg("--define")
                .arg(format!("_gpg_name {key}"))
                .arg("--addsign")
                .arg(rpm))
        } else {
            println!("(rpmsign is not installed: detached signature instead)");
            self.gpg_detach(rpm)
        }
    }

    /// A signed apt repository of the .deb in `target/dist/apt-repo`: `dists/stable/{Release,InRelease,Release.gpg}`, the pool, and the key.
    pub(super) fn apt_repo(&self) -> Result<(), String> {
        let key = self.key()?.to_string();
        let repo = self.dist().join("apt-repo");
        let deb = self.out().join(format!("{PKG}_{}_amd64.deb", self.deb_version()));
        if !deb.exists() {
            return Err(format!("{} is missing: run `cargo xtask dist deb` first", deb.display()));
        }
        let mut c = Command::new(self.root.join("tools/packaging/apt-repo.sh"));
        c.env("SOURCE_DATE_EPOCH", self.epoch()).arg(&repo).arg(&deb);
        sh(&mut c)?;
        let release = repo.join("dists/stable/Release");
        sh(Command::new("gpg")
            .args(["--batch", "--yes", "--local-user", &key, "--clearsign", "--output"])
            .arg(repo.join("dists/stable/InRelease"))
            .arg(&release))?;
        sh(Command::new("gpg")
            .args(["--batch", "--yes", "--armor", "--detach-sign", "--local-user", &key, "--output"])
            .arg(repo.join("dists/stable/Release.gpg"))
            .arg(&release))?;
        copy(&self.root.join(KEY_GPG), &repo.join("rusty-wave-release.gpg"))?;
        copy(&self.root.join(KEY_ASC), &repo.join("rusty-wave-release.asc"))?;
        println!("{}", repo.display());
        Ok(())
    }

    /// The release date as a unix time (`SOURCE_DATE_EPOCH` wins, else the last commit).
    fn epoch(&self) -> String {
        std::env::var("SOURCE_DATE_EPOCH")
            .ok()
            .or_else(|| {
                capture(Command::new("git").current_dir(&self.root).args(["log", "-1", "--format=%ct"])).ok()
            })
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    /// After a signed flatpak-builder run: the repo's default key, title, a signed summary, the appstream branch and static deltas; then
    /// `.flatpakrepo` (add the remote) and `.flatpakref` (install the app and add the remote) next to the other artifacts.
    pub(super) fn flatpak_publish(&self, repo: &Path) -> Result<(), String> {
        let key = self.key()?.to_string();
        let pubkey = self.root.join(KEY_GPG);
        let mut c = Command::new("flatpak");
        c.env("XDG_DATA_HOME", home().join(".local/share"))
            .env("XDG_CONFIG_HOME", home().join(".config"))
            .env("XDG_CACHE_HOME", home().join(".cache"))
            .arg("build-update-repo")
            .arg("--title=Rusty Wave")
            .arg("--comment=Rusty Wave (signed Flatpak repository)")
            .arg("--default-branch=stable")
            .arg(format!("--gpg-import={}", pubkey.display()))
            .arg(format!("--gpg-sign={key}"))
            .arg("--generate-static-deltas")
            .arg(repo);
        sh(&mut c)?;
        let url = self.repo_url.clone().unwrap_or_else(|| format!("file://{}", repo.display()));
        let gpg_b64 = capture(Command::new("base64").arg("-w0").arg(&pubkey))?.trim().to_string();
        let home_page = "https://github.com/unicorntearsproject/rusty-media-player";
        write(
            &self.out().join(format!("{APP_ID}.flatpakrepo")),
            format!(
                "[Flatpak Repo]\nTitle=Rusty Wave\nComment=Rusty Wave (signed Flatpak repository)\nUrl={url}\nHomepage={home_page}\nDefaultBranch=stable\nGPGKey={gpg_b64}\n"
            )
            .as_bytes(),
        )?;
        write(
            &self.out().join(format!("{APP_ID}.flatpakref")),
            format!(
                "[Flatpak Ref]\nTitle=Rusty Wave\nName={APP_ID}\nBranch=stable\nUrl={url}\nHomepage={home_page}\nIsRuntime=false\nSuggestRemoteName=rusty-wave\nRuntimeRepo=https://dl.flathub.org/repo/flathub.flatpakrepo\nGPGKey={gpg_b64}\n"
            )
            .as_bytes(),
        )?;
        println!("{url}");
        Ok(())
    }

    // ---- Checking ----------------------------------------------------------------------------------------------------------------

    /// Check every signature under `target/dist` against the public key in the repository, with a throwaway keyring and rpm database, so
    /// it proves what a user with only the published key sees. Missing artifacts are skipped; finding nothing signed is an error.
    pub(super) fn verify(&self) -> Result<(), String> {
        self.verify_with(true)
    }

    /// `verify`; `embedded` also requires the signatures inside the rpm and the AppImage (a release built in CI gets only detached ones).
    pub(super) fn verify_with(&self, embedded: bool) -> Result<(), String> {
        let fpr = resolve_public_fpr(&self.root)?;
        let guard = GpgHome::new(self.dist().join("verify-gnupg"))?;
        let home = guard.path().to_path_buf();
        let gpg = |args: &[&Path]| -> Result<String, String> {
            let o = Command::new("gpg")
                .env("GNUPGHOME", &home)
                .args(["--batch", "--no-tty", "--status-fd", "1", "--verify"])
                .args(args)
                .output()
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&o.stdout).into_owned();
            if o.status.success() && super::rbcheck::valid_sig_by(&text, &fpr) {
                Ok(text)
            } else {
                Err(format!("{}{}", text, String::from_utf8_lossy(&o.stderr)))
            }
        };
        let status = Command::new("gpg")
            .env("GNUPGHOME", &home)
            .args(["--batch", "--no-tty", "--quiet", "--import"])
            .arg(self.root.join(KEY_ASC))
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err("could not import the public key".into());
        }
        let (mut ok, mut bad) = (0, Vec::<String>::new());
        let mut check = |what: String, r: Result<(), String>| match r {
            Ok(()) => {
                ok += 1;
                println!("ok   {what}");
            }
            Err(e) => {
                println!("FAIL {what}: {}", e.trim());
                bad.push(what);
            }
        };
        let out = self.out();
        let mut names: Vec<PathBuf> = fs::read_dir(&out)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        names.sort();
        for f in &names {
            let name = f.file_name().unwrap().to_string_lossy().into_owned();
            if let Some(base) = name.strip_suffix(".asc") {
                let target = out.join(base);
                check(format!("gpg --verify {name}"), gpg(&[f, &target]).map(|_| ()));
            }
        }
        if out.join("SHA256SUMS.asc").exists() {
            let r = Command::new("sha256sum")
                .current_dir(&out)
                .args(["-c", "SHA256SUMS"])
                .output()
                .map_err(|e| e.to_string());
            check(
                "sha256sum -c SHA256SUMS".into(),
                r.and_then(|o| {
                    if o.status.success() {
                        Ok(())
                    } else {
                        Err(String::from_utf8_lossy(&o.stdout).into_owned())
                    }
                }),
            );
        }
        // rpm: embedded signature, checked against a private rpm database that holds only our key.
        let rpms: Vec<&PathBuf> =
            names.iter().filter(|p| p.extension().is_some_and(|e| e == "rpm")).collect();
        if embedded && !rpms.is_empty() && have("rpm") && have("rpmkeys") {
            let db = self.dist().join("verify-rpmdb");
            fs::create_dir_all(&db).map_err(|e| e.to_string())?;
            sh(Command::new("rpmkeys")
                .arg("--dbpath")
                .arg(&db)
                .arg("--import")
                .arg(self.root.join(KEY_ASC)))?;
            for r in rpms {
                let o = capture(Command::new("rpm").arg("--dbpath").arg(&db).arg("-Kv").arg(r));
                let name = r.file_name().unwrap().to_string_lossy().into_owned();
                check(
                    format!("rpm -K {name}"),
                    o.and_then(|t| {
                        if release_fprs(&self.root)
                            .is_ok_and(|all| all.iter().any(|f| t.contains(f.as_str())))
                            && t.contains("signature")
                            && !t.contains("NOT OK")
                            && !t.contains("BAD")
                        {
                            Ok(())
                        } else {
                            Err(format!("no valid embedded signature by {fpr}:\n{t}"))
                        }
                    }),
                );
            }
        }
        // AppImage: the signature appimagetool embeds.
        for f in names.iter().filter(|p| embedded && p.extension().is_some_and(|e| e == "AppImage")) {
            let r = capture(
                Command::new("python3")
                    .arg(self.root.join("tools/packaging/verify-appimage-sig.py"))
                    .arg(f)
                    .arg(&fpr)
                    .arg(self.root.join(KEY_ASC)),
            );
            check(format!("embedded signature {}", f.file_name().unwrap().to_string_lossy()), r.map(|_| ()));
        }
        // AppImage update information and the .zsync: the file embeds the stable alias of the zsync file, and the zsync file downloads the
        // versioned AppImage and describes exactly the final (signed) file.
        for f in names.iter().filter(|p| p.extension().is_some_and(|e| e == "AppImage")) {
            let name = f.file_name().unwrap().to_string_lossy().into_owned();
            let want = self.update_info();
            let r = fs::read(f).map_err(|e| e.to_string()).and_then(|b| {
                let w = want.as_bytes();
                b.windows(w.len())
                    .any(|x| x == w)
                    .then_some(())
                    .ok_or_else(|| format!("`{want}` is not embedded"))
            });
            check(format!("update information in {name}"), r);
            let zs = out.join(format!("{name}.zsync"));
            if zs.is_file() {
                let r = fs::read(&zs).map_err(|e| e.to_string()).and_then(|b| {
                    let head = String::from_utf8_lossy(&b[..b.len().min(2048)]).into_owned();
                    let size = fs::metadata(f).map_err(|e| e.to_string())?.len();
                    let sha1 = capture(Command::new("sha1sum").arg(f))?;
                    let url = format!("{}/{name}", self.versioned_base());
                    manifest::check_zsync(
                        &head,
                        &url,
                        size,
                        sha1.split_whitespace().next().unwrap_or_default(),
                    )
                });
                check(format!("{name}.zsync matches {name}"), r);
            } else {
                check(format!("{name}.zsync matches {name}"), Err("missing (run `dist appimage`)".into()));
            }
        }
        // The files of the other platforms: signed like the rest above, and each has to be what its name says.
        for f in &names {
            let name = f.file_name().unwrap().to_string_lossy().into_owned();
            if name.ends_with(".dmg") {
                // A UDIF disk image ends with a 512-byte trailer that starts with `koly`.
                let r = fs::read(f).map_err(|e| e.to_string()).and_then(|b| {
                    (b.len() > 512 && &b[b.len() - 512..b.len() - 508] == b"koly")
                        .then_some(())
                        .ok_or_else(|| "no UDIF `koly` trailer: not a disk image".to_string())
                });
                check(format!("{name} is a disk image"), r);
            } else if name.ends_with(".flatpak") {
                // A bundle is an OSTree commit with the ref in its header.
                let want = format!("app/{APP_ID}/x86_64/stable");
                let r = fs::read(f).map_err(|e| e.to_string()).and_then(|b| {
                    let w = want.as_bytes();
                    b.windows(w.len())
                        .any(|x| x == w)
                        .then_some(())
                        .ok_or_else(|| format!("the bundle does not name the ref {want}"))
                });
                check(format!("{name} is a bundle of {want}"), r);
                // GNOME Software and Discover show a bundle's icon from its header (the 64 and 128 pixel icons of the appstream data) before
                // it is installed: a bundle built without them is shown with the generic icon.
                let r = fs::read(f).map_err(|e| e.to_string()).and_then(|b| {
                    let sizes = png_sizes(&b[..b.len().min(1 << 16)]);
                    [(64, 64), (128, 128)].iter().all(|s| sizes.contains(s)).then_some(()).ok_or_else(|| {
                        format!("the bundle header holds icons of {sizes:?}, not 64x64 and 128x128")
                    })
                });
                check(format!("{name} carries its icon"), r);
            } else if name.ends_with(".AppImage") {
                // File managers and AppImageLauncher read `.DirIcon` out of the image for its thumbnail.
                let work = self.dist().join("verify-appimage-icon");
                let r = fs::create_dir_all(&work).map_err(|e| e.to_string()).and_then(|()| {
                    sh(Command::new(f).current_dir(&work).args(["--appimage-extract", ".DirIcon"]))?;
                    let b = fs::read(work.join("squashfs-root/.DirIcon"))
                        .map_err(|e| format!(".DirIcon: {e}"))?;
                    let sizes = png_sizes(&b);
                    sizes.iter().any(|&(w, h)| w >= 128 && h >= 128).then_some(()).ok_or_else(|| {
                        format!(".DirIcon is {sizes:?}, expected a PNG of at least 128 pixels")
                    })
                });
                check(format!("{name} carries its .DirIcon"), r);
            } else if name.ends_with(".zip") {
                let script = "import sys,zipfile; bad=zipfile.ZipFile(sys.argv[1]).testzip(); sys.exit(f'corrupt member {bad}' if bad else 0)";
                let r = capture(Command::new("python3").args(["-c", script]).arg(f));
                check(format!("{name} is an intact zip"), r.map(|_| ()));
            }
        }
        // apt repo: InRelease (clearsigned) and Release.gpg (detached), and the hashes they cover.
        let apt = self.dist().join("apt-repo/dists/stable");
        if apt.join("Release").exists() {
            check("gpg --verify apt InRelease".into(), gpg(&[&apt.join("InRelease")]).map(|_| ()));
            check(
                "gpg --verify apt Release.gpg".into(),
                gpg(&[&apt.join("Release.gpg"), &apt.join("Release")]).map(|_| ()),
            );
            let release = fs::read_to_string(apt.join("Release")).map_err(|e| e.to_string())?;
            let packages = apt.join("main/binary-amd64/Packages");
            let want = sha256(&packages)?;
            check(
                "apt Release lists the Packages hash".into(),
                release.contains(&want).then_some(()).ok_or_else(|| "hash mismatch".to_string()),
            );
        }
        // Flatpak repo: the summary is signed and the app commit carries a signature by our key (ostree cannot check it against a throwaway
        // keyring; the real check is installing from the remote, which verifies it: see docs/release-testing.md).
        let fp = self.staging().join("flatpak/repo");
        if fp.join("summary.sig").exists() && have("ostree") {
            let sig_len = fs::metadata(fp.join("summary.sig")).map(|m| m.len()).unwrap_or(0);
            check(
                "flatpak summary.sig present".into(),
                (sig_len > 0).then_some(()).ok_or_else(|| "empty".to_string()),
            );
            let shown = capture(
                Command::new("ostree")
                    .arg(format!("--repo={}", fp.display()))
                    .args(["show", &format!("app/{APP_ID}/x86_64/stable")]),
            );
            let keyid = fpr[fpr.len() - 16..].to_string();
            check(
                "flatpak commit signed by the release key".into(),
                shown.and_then(|t| {
                    t.contains(&keyid).then_some(()).ok_or_else(|| format!("no signature by {keyid}"))
                }),
            );
        }
        if ok == 0 && bad.is_empty() {
            return Err("nothing signed under target/dist (build with --sign)".into());
        }
        println!("{ok} ok, {} failed (key {fpr})", bad.len());
        if bad.is_empty() { Ok(()) } else { Err(format!("failed: {}", bad.join(", "))) }
    }
}

/// Every fingerprint in the repository's public key, lower-case: the primary key and its signing subkeys (a signature made by a subkey
/// shows the subkey's, not the primary's).
pub(super) fn release_fprs(root: &Path) -> Result<Vec<String>, String> {
    let out = capture(
        Command::new("gpg").args(["--batch", "--show-keys", "--with-colons"]).arg(root.join(KEY_ASC)),
    )?;
    Ok(out
        .lines()
        .filter(|l| l.starts_with("fpr:"))
        .filter_map(|l| l.split(':').nth(9))
        .map(str::to_lowercase)
        .collect())
}

/// Width and height of every PNG whose signature is found in `bytes`.
pub(super) fn png_sizes(bytes: &[u8]) -> Vec<(u32, u32)> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    let mut out = Vec::new();
    let mut i = 0;
    while i + 24 <= bytes.len() {
        if &bytes[i..i + 8] == SIG {
            let be = |o: usize| {
                u32::from_be_bytes([bytes[i + o], bytes[i + o + 1], bytes[i + o + 2], bytes[i + o + 3]])
            };
            out.push((be(16), be(20)));
            i += 8;
        } else {
            i += 1;
        }
    }
    out
}

/// The fingerprint of the public key in the repository.
pub(super) fn resolve_public_fpr(root: &Path) -> Result<String, String> {
    fingerprint(&capture(
        Command::new("gpg").args(["--batch", "--show-keys", "--with-colons"]).arg(root.join(KEY_ASC)),
    )?)
    .ok_or_else(|| format!("no key in {KEY_ASC}"))
}

/// A throwaway GnuPG home. Whatever gpg started there (gpg-agent, dirmngr) is stopped when this goes out of scope, on success, on an error
/// return and on a panic alike.
pub(super) struct GpgHome(PathBuf);

impl GpgHome {
    pub(super) fn new(path: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        set_mode(&path, 0o700)?;
        Ok(GpgHome(path))
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for GpgHome {
    fn drop(&mut self) {
        let _ = Command::new("gpgconf").arg("--homedir").arg(&self.0).args(["--kill", "all"]).output();
    }
}

#[cfg(test)]
mod icon_tests {
    use super::png_sizes;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0]);
        v
    }

    #[test]
    fn the_sizes_of_the_pngs_in_a_header_are_read() {
        let mut b = b"flatpak\0header".to_vec();
        b.extend(png(64, 64));
        b.extend_from_slice(b"padding");
        b.extend(png(128, 128));
        assert_eq!(png_sizes(&b), [(64, 64), (128, 128)]);
        assert!(png_sizes(b"no pictures here").is_empty());
        assert!(png_sizes(&png(1, 1)[..10]).is_empty(), "a cut-off signature is not a picture");
    }
}
