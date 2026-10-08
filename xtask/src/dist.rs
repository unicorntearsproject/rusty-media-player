//! `cargo xtask dist <target>`: release packages of the desktop app and the PWA. See `docs/release/packaging.md`.
//!
//! Everything here shells out to the packaging tools (cargo-deb, cargo-generate-rpm, appimagetool, flatpak-builder, podman, wine, Inno
//! Setup); there are no secrets in it, and signing is `--sign` (the GPG release key in the user's keyring, see `sign.rs`) or a hook
//! (`RVP_SIGN_CMD`, `RVP_WINDOWS_SIGN_CMD`). Nothing is deleted recursively:
//! staging directories are overwritten in place.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

mod macos;
mod manifest;
mod publish;
mod rbcheck;
mod sign;

const APP_ID: &str = "io.github.unicorntearsproject.RustyWave";
const PKG: &str = "rusty-wave";
/// The image the Linux binary is built in (Ubuntu 22.04: glibc 2.35, so it runs on that and anything newer).
const LINUX_IMAGE: &str = "localhost/rvp-build-linux:1";
/// The image that cross-builds the Windows exe and runs Inno Setup under wine.
const WIN_IMAGE: &str = "localhost/rvp-win:1";
const INNO_VERSION: &str = "6.7.3";
const INNO_URL: &str = "https://github.com/jrsoftware/issrc/releases/download/is-6_7_3/innosetup-6.7.3.exe";
const INNO_SHA256: &str = "9c73c3bae7ed48d44112a0f48e66742c00090bdb5bef71d9d3c056c66e97b732";
const APPIMAGETOOL_URL: &str =
    "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage";

pub const USAGE: &str = "usage: cargo xtask dist <target> [--version V] [--container] [--no-build]

targets:
  linux-bin    build the Linux binary (profile `dist`); --container builds it in the Ubuntu 22.04 image (portable glibc)
  stage        the installed tree (usr/...) with the stamped metadata, in target/dist/linux/stage
  tarball      rusty-wave-<ver>-linux-x86_64.tar.gz of that tree (and the source tarball the Flatpak builds from)
  deb          rusty-wave_<ver>_amd64.deb (cargo-deb)
  rpm          rusty-wave-<ver>-<release>.x86_64.rpm (cargo-generate-rpm; 1.0.0-rc1 is 1.0.0-0.1.rc1)
  appimage     rusty-wave-<ver>-x86_64.AppImage (appimagetool, with update information) and its .zsync (zsyncmake)
  flatpak-sources   regenerate packaging/flatpak/cargo-sources.json from Cargo.lock (flatpak-cargo-generator)
  flatpak      build the Flatpak with flatpak-builder from the working tree and bundle it (.flatpak);
               --prepare-only just writes the manifest and the source tarball (CI builds it with the flatpak-builder action)
  windows      rusty-wave.exe (x86_64-pc-windows-gnu in the wine image on Linux; the host toolchain on Windows) and the portable zip
  installer    the Inno Setup installer around it (ISCC.exe on Windows, wine in the image on Linux)
  macos        Rusty Wave.app (universal arm64 + x86_64) in rusty-wave-<ver>-macos-universal.dmg; macOS only. Signing hooks:
               RVP_MACOS_SIGN_IDENTITY (Developer ID, hardened runtime) and RVP_MACOS_NOTARY_PROFILE (notarytool keychain profile);
               with neither the bundle is ad-hoc signed and the dmg unsigned
  pwa          the web app (cargo xtask web) as rusty-wave-web-<ver>.zip
  apt-repo     a signed apt repository of the .deb in target/dist/apt-repo (needs --sign)
  checksums    SHA256SUMS over everything in target/dist/release (and signatures if --sign or RVP_SIGN_CMD is set)
  verify       check every signature in target/dist against packaging/keys/rusty-wave-release.asc in a throwaway keyring;
               with --target rustybucket: check the directory `publish --target rustybucket` staged against Rusty Bucket's input contract
  manifest     rusty-wave-latest.json (the in-app updater's manifest) of what is built in target/dist/release; --base-url U sets where the
               files will be served (an https:// URL or file:///path, default Rusty Bucket's release site); --windows, --macos, --flatpak, --web, --tarball (or --all) list those files too
  publish      lay the verified release out for Rusty Bucket's release site (software.rustybucket.ai) in target/dist/publish-rb/rusty-wave: <version>/ (the versioned
               files), latest/ (aliases and the signed manifest) and release/ (the directory their publisher takes), and check it against their input
               contract; needs --sign, uploads nothing; --windows / --macos / --flatpak / --web / --tarball (or --all) add the Windows installer and zip / the macOS
               dmg / the Flatpak bundle / the web app zip / the Linux tarball (only when built and verified in the same run); --dry-run only checks
  check        validate the metadata (desktop file, AppStream, man page) without building anything
  linux        stage, tarball, deb, rpm, appimage and flatpak
  all          linux, windows, installer, pwa and checksums

--version V   stamp V instead of the version in Cargo.toml (a dry run: `0.0.0-ci1`); the deb and rpm keep Cargo's version
--container   build the Linux binary in the Ubuntu 22.04 image (needs podman); CI builds on an ubuntu-22.04 runner instead
--no-build    use the binary and stage that are already there
--sign        sign with the release key: rpm (rpmsign), AppImage (embedded), the Flatpak repo and its commit, the apt repo's Release,
              and a detached .asc next to every artifact and SHA256SUMS. The key is RVP_GPG_KEY (a fingerprint) or the one in
              packaging/keys/rusty-wave-release.asc; its secret half must be in your gpg keyring.
--sign-key K  like --sign with the key K
--base-url U  where the published files are served from (the AppImage's update information, the manifest); default Rusty Bucket's release site
--repo-url U  the URL the Flatpak repo will be served from, written into the .flatpakrepo and .flatpakref (default file://<local repo>)

Outputs go to target/dist/release. Other signing hooks: RVP_SIGN_CMD (run once per Linux artifact with {} replaced by its path),
RVP_WINDOWS_SIGN_CMD (an Inno Setup SignTool command, with $f for the file). No secret keys live in the repository.";

struct Ctx {
    prepare_only: bool,
    root: PathBuf,
    version: String,
    date: String,
    container: bool,
    no_build: bool,
    /// The fingerprint to sign with (`--sign`), already checked to be in the keyring.
    sign: Option<String>,
    repo_url: Option<String>,
    base_url: Option<String>,
    /// `--target rustybucket` (the only target): with `verify`, check the staged release directory against Rusty Bucket's contract.
    rustybucket: bool,
}

pub fn run(args: &[String]) -> Result<(), String> {
    let Some(target) = args.first() else { return Err(USAGE.into()) };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").canonicalize().map_err(|e| e.to_string())?;
    // Windows canonicalizes to `\\?\D:\...`, which Inno Setup (and most tools) reject: use the plain path.
    let root = match root.to_string_lossy().strip_prefix(r"\\?\") {
        Some(plain) => PathBuf::from(plain),
        None => root,
    };
    let mut version = None;
    let (mut container, mut no_build, mut prepare_only) = (false, false, false);
    let (mut want_sign, mut sign_key, mut repo_url) = (false, None, None);
    let (mut extras, mut dry_run) = (manifest::Extras::default(), false);
    let (mut base_url, mut rustybucket) = (None, false);
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--version" => version = Some(it.next().ok_or("--version needs a value")?.clone()),
            "--container" => container = true,
            "--no-build" => no_build = true,
            "--prepare-only" => prepare_only = true,
            "--windows" => extras.windows = true,
            "--macos" => extras.macos = true,
            "--flatpak" => extras.flatpak = true,
            "--web" => extras.web = true,
            "--tarball" => extras.tarball = true,
            "--all" => extras = manifest::Extras::ALL,
            "--base-url" => base_url = Some(it.next().ok_or("--base-url needs a value")?.clone()),
            "--target" => {
                rustybucket = match it.next().ok_or("--target needs a value")?.as_str() {
                    "rustybucket" => true,
                    other => {
                        return Err(format!(
                            "unknown --target `{other}`: rustybucket is the only target (the old bucket was dropped)"
                        ));
                    }
                }
            }
            "--dry-run" => dry_run = true,
            "--sign" => want_sign = true,
            "--sign-key" => {
                want_sign = true;
                sign_key = Some(it.next().ok_or("--sign-key needs a value")?.clone());
            }
            "--repo-url" => repo_url = Some(it.next().ok_or("--repo-url needs a value")?.clone()),
            other => return Err(format!("unknown option `{other}`\n\n{USAGE}")),
        }
    }
    let cargo_version = workspace_version(&root)?;
    let version = version
        .or_else(|| std::env::var("RVP_VERSION").ok().filter(|v| !v.is_empty()))
        .unwrap_or(cargo_version);
    let date = release_date(&root);
    let sign = if want_sign { Some(sign::resolve_key(&root, sign_key)?) } else { None };
    let cx =
        Ctx { root, version, date, container, no_build, prepare_only, sign, repo_url, base_url, rustybucket };
    fs::create_dir_all(cx.out()).map_err(|e| e.to_string())?;
    match target.as_str() {
        "linux-bin" => cx.linux_bin().map(|_| ()),
        "stage" => cx.stage(),
        "tarball" => cx.tarball(),
        "deb" => cx.deb(),
        "rpm" => cx.rpm(),
        "appimage" => cx.appimage(),
        "flatpak-sources" => cx.flatpak_sources(),
        "flatpak" => cx.flatpak(),
        "windows" => cx.windows(),
        "installer" => cx.installer(),
        "pwa" => cx.pwa(),
        "checksums" => cx.checksums(),
        "apt-repo" => cx.apt_repo(),
        "verify" if cx.rustybucket => cx.verify_rustybucket(),
        "verify" => cx.verify(),
        "macos" => cx.macos(),
        "manifest" => cx.manifest(extras),
        "publish" => cx.publish(extras, dry_run),
        "check" => cx.check(),
        "linux" => {
            cx.stage()?;
            cx.tarball()?;
            cx.deb()?;
            cx.rpm()?;
            cx.appimage()?;
            cx.flatpak()?;
            if cx.sign.is_some() {
                cx.apt_repo()?;
            }
            Ok(())
        }
        "all" => {
            cx.stage()?;
            cx.tarball()?;
            cx.deb()?;
            cx.rpm()?;
            cx.appimage()?;
            cx.flatpak()?;
            cx.windows()?;
            cx.installer()?;
            cx.pwa()?;
            if cx.sign.is_some() {
                cx.apt_repo()?;
            }
            cx.checksums()
        }
        other => Err(format!("unknown dist target `{other}`\n\n{USAGE}")),
    }
}

/// The rpm version and release of a version: `1.0.0-rc1` is version `1.0.0`, release `0.1.rc1`, which sorts before `1.0.0-1` (the
/// release) and `rc2` (`0.1.rc2`) after it; a plain version is release `1`.
fn rpm_parts(version: &str) -> (String, String) {
    match version.split_once('-') {
        Some((v, pre)) => (v.to_string(), format!("0.1.{}", pre.replace('-', "."))),
        None => (version.to_string(), "1".to_string()),
    }
}

/// What the Flatpak needs to be allowed to do for the features that touch the outside (checked, so a manifest edit cannot quietly take one away):
/// the user's music and videos read and write (the tag editor), the network (a theme link), and the host's `mimeapps.list` ("Set as default
/// media player"; Flatpak's own `XDG_CONFIG_HOME` is a private copy the desktop never reads).
fn flatpak_permissions_ok(manifest: &str) -> Result<(), String> {
    let args: Vec<&str> =
        manifest.lines().filter_map(|l| l.trim().strip_prefix("- --")).map(str::trim).collect();
    for want in [
        "filesystem=xdg-music",
        "filesystem=xdg-videos",
        "share=network",
        "filesystem=xdg-config/mimeapps.list",
    ] {
        if !args.contains(&want) {
            return Err(format!("the Flatpak manifest does not grant --{want}"));
        }
    }
    if let Some(a) = args.iter().find(|a| a.starts_with("filesystem=") && a.ends_with(":ro")) {
        return Err(format!(
            "the Flatpak manifest grants --{a}: the tag editor needs write access to music and videos"
        ));
    }
    Ok(())
}

/// `version = "x"` under `[workspace.package]`.
fn workspace_version(root: &Path) -> Result<String, String> {
    let text = fs::read_to_string(root.join("Cargo.toml")).map_err(|e| e.to_string())?;
    let mut in_pkg = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_pkg = l == "[workspace.package]";
        } else if in_pkg && l.starts_with("version") {
            return l.split('"').nth(1).map(str::to_string).ok_or_else(|| "bad version line".to_string());
        }
    }
    Err("no [workspace.package] version in Cargo.toml".into())
}

/// The date of the last commit (reproducible), else today.
fn release_date(root: &Path) -> String {
    if let Ok(d) = std::env::var("SOURCE_DATE_EPOCH") {
        if let Ok(o) = Command::new("date").args(["-u", "-d", &format!("@{d}"), "+%F"]).output() {
            if o.status.success() {
                return String::from_utf8_lossy(&o.stdout).trim().to_string();
            }
        }
    }
    let o = Command::new("git").current_dir(root).args(["log", "-1", "--format=%cs"]).output();
    match o {
        Ok(o) if o.status.success() && !o.stdout.is_empty() => {
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        }
        _ => String::from_utf8_lossy(
            &Command::new("date").arg("+%F").output().map(|o| o.stdout).unwrap_or_default(),
        )
        .trim()
        .to_string(),
    }
}

fn sh(cmd: &mut Command) -> Result<(), String> {
    println!("+ {cmd:?}");
    let st = cmd.status().map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    st.success().then_some(()).ok_or_else(|| format!("{:?} failed ({st})", cmd.get_program()))
}

fn capture(cmd: &mut Command) -> Result<String, String> {
    let o = cmd.output().map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    if !o.status.success() {
        return Err(format!("{:?} failed: {}", cmd.get_program(), String::from_utf8_lossy(&o.stderr)));
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

fn have(tool: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool}"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Podman, with the XDG variables of a normal desktop session (an IDE's snap sandbox changes them and podman refuses its own storage).
fn podman() -> Command {
    let mut c = Command::new("podman");
    let h = home();
    c.env("XDG_DATA_HOME", h.join(".local/share"))
        .env("XDG_CONFIG_HOME", h.join(".config"))
        .env("XDG_CACHE_HOME", h.join(".cache"));
    if let Some(r) = std::env::var_os("XDG_RUNTIME_DIR") {
        c.env("XDG_RUNTIME_DIR", r);
    } else {
        c.env("XDG_RUNTIME_DIR", "/run/user/1000");
    }
    c
}

fn image_exists(image: &str) -> bool {
    podman().args(["image", "exists", image]).status().is_ok_and(|s| s.success())
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(d) = to.parent() {
        fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    fs::copy(from, to).map_err(|e| format!("copy {} -> {}: {e}", from.display(), to.display()))?;
    Ok(())
}

fn write(to: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(d) = to.parent() {
        fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    fs::write(to, data).map_err(|e| format!("{}: {e}", to.display()))
}

fn sha256(path: &Path) -> Result<String, String> {
    let out = capture(Command::new("sha256sum").arg(path))?;
    Ok(out.split_whitespace().next().unwrap_or_default().to_string())
}

impl Ctx {
    fn dist(&self) -> PathBuf {
        self.root.join("target/dist")
    }

    /// Build staging that holds symlinks (the Wine prefix links `z:` to `/`, the Flatpak build root and virtualenv link into the system), kept out
    /// of the repository so that tools that walk it (editors, `rg --follow`) never follow them. `RVP_STAGING_DIR` overrides the place.
    fn staging(&self) -> PathBuf {
        if let Some(d) = std::env::var_os("RVP_STAGING_DIR") {
            return PathBuf::from(d);
        }
        // The scratch disk where there is one (the root drive fills up), else the user's cache folder.
        let scratch = Path::new("/mnt/scratch");
        if scratch.is_dir() {
            return scratch.join("rvp-scratch/staging");
        }
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_else(std::env::temp_dir);
        cache.join("rvp-scratch/staging")
    }

    fn out(&self) -> PathBuf {
        self.dist().join("release")
    }

    fn stage_dir(&self) -> PathBuf {
        self.dist().join("linux/stage")
    }

    fn stamp(&self, text: &str) -> String {
        text.replace("@VERSION@", &self.version).replace("@DATE@", &self.date)
    }

    /// The metainfo file: the release is stamped with the version as AppStream orders it.
    fn stamp_metainfo(&self, text: &str) -> String {
        text.replace("@VERSION@", &self.appstream_version()).replace("@DATE@", &self.date)
    }

    /// The version as AppStream orders it: a pre-release is marked with `~` (`1.0.0~rc1` sorts before `1.0.0`), like Debian's.
    fn appstream_version(&self) -> String {
        self.version.replacen('-', "~", 1)
    }

    /// The commit the build is made from (the About page shows it); a container has no git of its own, so it is handed in.
    fn build_commit(&self) -> String {
        std::env::var("RVP_BUILD_COMMIT")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                capture(Command::new("git").current_dir(&self.root).args(["rev-parse", "--short=10", "HEAD"]))
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| "unknown".into())
    }

    fn cargo_cmd(&self) -> Command {
        // Packages are built with a stable toolchain when `rustup` has one pinned for this (RVP_TOOLCHAIN, default 1.99.0).
        let tc = std::env::var("RVP_TOOLCHAIN").unwrap_or_else(|_| "1.99.0".into());
        let mut c = Command::new("cargo");
        if !tc.is_empty()
            && capture(Command::new("rustup").args(["toolchain", "list"])).is_ok_and(|l| l.contains(&tc))
        {
            c.arg(format!("+{tc}"));
        }
        c.current_dir(&self.root);
        c
    }

    // ---- Linux binary and the installed tree -----------------------------------------------------------------------------------

    fn linux_bin(&self) -> Result<PathBuf, String> {
        let out = self.dist().join("linux/rusty-wave");
        if self.no_build && out.exists() {
            return Ok(out);
        }
        let built = if self.container {
            if !image_exists(LINUX_IMAGE) {
                let base = std::env::var("RVP_BASE_IMAGE")
                    .unwrap_or_else(|_| "docker.io/library/ubuntu:22.04".into());
                sh(podman().current_dir(self.root.join("packaging/linux")).args([
                    "build",
                    "--build-arg",
                    &format!("BASE={base}"),
                    "-f",
                    "Containerfile.build",
                    "-t",
                    LINUX_IMAGE,
                    ".",
                ]))?;
            }
            let cache = self.dist().join("cargo-home");
            fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
            sh(podman().args([
                "run",
                "--rm",
                "--security-opt",
                "label=disable",
                "-v",
                &format!("{}:/work", self.root.display()),
                "-v",
                &format!("{}:/cargo-cache", cache.display()),
                "-e",
                &format!("RVP_BUILD_COMMIT={}", self.build_commit()),
                "-e",
                "CARGO_HOME=/cargo-cache",
                "-e",
                "RUSTUP_HOME=/opt/rustup",
                "-w",
                "/work",
                LINUX_IMAGE,
                "/opt/cargo/bin/cargo",
                "build",
                "--profile",
                "dist",
                "-p",
                "rvp-host-desktop",
                "--target-dir",
                "/work/target/dist-linux",
                "--locked",
            ]))?;
            self.root.join("target/dist-linux/dist/rusty-wave")
        } else {
            sh(self.cargo_cmd().args(["build", "--profile", "dist", "-p", "rvp-host-desktop", "--locked"]))?;
            self.root.join("target/dist/rusty-wave")
        };
        copy(&built, &out)?;
        Ok(out)
    }

    /// The tree the Linux packages install, under `stage/usr`.
    fn stage(&self) -> Result<(), String> {
        let bin = self.linux_bin()?;
        let s = self.stage_dir().join("usr");
        let shared = self.root.join("packaging/shared");
        copy(&bin, &s.join("bin/rusty-wave"))?;
        set_mode(&s.join("bin/rusty-wave"), 0o755)?;
        copy(
            &shared.join(format!("{APP_ID}.desktop")),
            &s.join(format!("share/applications/{APP_ID}.desktop")),
        )?;
        let meta = fs::read_to_string(shared.join(format!("{APP_ID}.metainfo.xml.in")))
            .map_err(|e| e.to_string())?;
        write(
            &s.join(format!("share/metainfo/{APP_ID}.metainfo.xml")),
            self.stamp_metainfo(&meta).as_bytes(),
        )?;
        let icons = self.root.join("packaging/icons/hicolor");
        for size in [16, 22, 24, 32, 48, 64, 96, 128, 192, 256, 512] {
            let rel = format!("hicolor/{size}x{size}/apps/{APP_ID}.png");
            copy(&icons.join(format!("{size}x{size}/apps/{APP_ID}.png")), &s.join("share/icons").join(rel))?;
        }
        // The same icon in pixmaps: the place older software centres (and the local-package path of some) look for the icon of a package
        // that is not installed yet, since the icon theme cannot name it before it is.
        copy(
            &icons.join(format!("128x128/apps/{APP_ID}.png")),
            &s.join(format!("share/pixmaps/{APP_ID}.png")),
        )?;
        // The man page, gzipped (-n: no timestamp, so the package is reproducible).
        let man = fs::read_to_string(shared.join("rusty-wave.1.in")).map_err(|e| e.to_string())?;
        let man_path = s.join("share/man/man1/rusty-wave.1");
        write(&man_path, self.stamp(&man).as_bytes())?;
        sh(Command::new("gzip").args(["-n", "-9", "-f"]).arg(&man_path))?;
        let doc = s.join("share/doc").join(PKG);
        copy(&self.root.join("README.md"), &doc.join("README.md"))?;
        copy(&self.root.join("THIRD_PARTY_LICENSES.md"), &doc.join("THIRD_PARTY_LICENSES.md"))?;
        copy(&self.root.join("LICENSE-MIT"), &doc.join("LICENSE-MIT"))?;
        copy(&self.root.join("LICENSE-APACHE"), &doc.join("LICENSE-APACHE"))?;
        copy(&self.root.join("packaging/linux/copyright"), &doc.join("copyright"))?;
        copy(&self.root.join("LICENSE-MIT"), &s.join("share/licenses").join(PKG).join("LICENSE-MIT"))?;
        copy(&self.root.join("LICENSE-APACHE"), &s.join("share/licenses").join(PKG).join("LICENSE-APACHE"))?;
        println!("staged {}", self.stage_dir().display());
        Ok(())
    }

    fn tarball(&self) -> Result<(), String> {
        if !self.no_build || !self.stage_dir().join("usr/bin/rusty-wave").exists() {
            self.stage()?;
        }
        let name = format!("rusty-wave-{}-linux-x86_64", self.version);
        let out = self.out().join(format!("{name}.tar.gz"));
        sh(Command::new("tar")
            .current_dir(self.stage_dir())
            .args(["--owner=0", "--group=0", "--sort=name", "--transform"])
            .arg(format!("s,^usr,{name},"))
            .arg("-czf")
            .arg(&out)
            .arg("usr"))?;
        // The source the Flatpak (and anyone else) builds from: the tracked and the new, not-ignored files of the working tree.
        let src = self.source_tarball()?;
        println!("{}\n{}", out.display(), src.display());
        Ok(())
    }

    fn source_tarball(&self) -> Result<PathBuf, String> {
        let list = capture(Command::new("git").current_dir(&self.root).args([
            "ls-files",
            "-co",
            "--exclude-standard",
        ]))?;
        let files: Vec<&str> =
            list.lines().filter(|f| self.root.join(f).is_file() && !f.starts_with(".claude/")).collect();
        let list_file = self.dist().join("source-files.txt");
        write(&list_file, files.join("\n").as_bytes())?;
        let out = self.out().join(format!("{PKG}-{}-src.tar.gz", self.version));
        sh(Command::new("tar")
            .current_dir(&self.root)
            .args(["--owner=0", "--group=0", "--sort=name", "--transform"])
            .arg(format!("s,^,{PKG}-{}/,", self.version))
            .arg("-czf")
            .arg(&out)
            .arg("-T")
            .arg(&list_file))?;
        Ok(out)
    }

    // ---- .deb and .rpm -----------------------------------------------------------------------------------------------------------

    fn ensure_stage(&self) -> Result<(), String> {
        if self.no_build && self.stage_dir().join("usr/bin/rusty-wave").exists() {
            return Ok(());
        }
        self.stage()
    }

    fn deb(&self) -> Result<(), String> {
        self.ensure_stage()?;
        if !have("cargo-deb") {
            return Err("cargo-deb is missing: cargo install cargo-deb".into());
        }
        let mut c = self.cargo_cmd();
        c.args(["deb", "-p", "rvp-host-desktop", "--no-build", "--no-strip", "--profile", "dist", "-o"])
            .arg(self.out().join(format!("{PKG}_{}_amd64.deb", self.deb_version())));
        // The stamp may differ from Cargo's version on a dry run; the deb carries what it is told.
        c.arg("--deb-version").arg(self.deb_version());
        sh(&mut c)?;
        if self.sign.is_some() {
            self.sign_deb(&self.out().join(format!("{PKG}_{}_amd64.deb", self.deb_version())))?;
        }
        Ok(())
    }

    /// Debian does not like `-rc1` style tags to sort after the release: `0.1.0-rc1` becomes `0.1.0~rc1`.
    fn deb_version(&self) -> String {
        self.version.replacen('-', "~", 1)
    }

    fn rpm(&self) -> Result<(), String> {
        self.ensure_stage()?;
        if !have("cargo-generate-rpm") {
            return Err("cargo-generate-rpm is missing: cargo install cargo-generate-rpm".into());
        }
        let (ver, rel) = rpm_parts(&self.version);
        let rpm = self.out().join(format!("{PKG}-{ver}-{rel}.x86_64.rpm"));
        let mut c = self.cargo_cmd();
        c.args(["generate-rpm", "-p", "crates/rvp-host-desktop", "--profile", "dist", "-o"])
            .arg(&rpm)
            .arg("--set-metadata")
            .arg(format!("version = \"{ver}\""))
            .arg("--set-metadata")
            .arg(format!("release = \"{rel}\""));
        sh(&mut c)?;
        self.sign_rpm(&rpm)
    }

    // ---- AppImage ----------------------------------------------------------------------------------------------------------------

    fn appimagetool(&self) -> Result<PathBuf, String> {
        if let Some(p) = std::env::var_os("APPIMAGETOOL") {
            return Ok(PathBuf::from(p));
        }
        let local = home().join(".local/bin/appimagetool");
        if local.exists() {
            return Ok(local);
        }
        if have("appimagetool") {
            return Ok(PathBuf::from("appimagetool"));
        }
        fs::create_dir_all(local.parent().unwrap()).map_err(|e| e.to_string())?;
        sh(Command::new("curl").args(["-fsSL", "-o"]).arg(&local).arg(APPIMAGETOOL_URL))?;
        set_mode(&local, 0o755)?;
        Ok(local)
    }

    fn appimage(&self) -> Result<(), String> {
        self.ensure_stage()?;
        let tool = self.appimagetool()?;
        let dir = self.dist().join("appimage/AppDir");
        let stage = self.stage_dir().join("usr");
        copy(&stage.join("bin/rusty-wave"), &dir.join("usr/bin/rusty-wave"))?;
        set_mode(&dir.join("usr/bin/rusty-wave"), 0o755)?;
        for rel in [
            format!("share/applications/{APP_ID}.desktop"),
            format!("share/icons/hicolor/256x256/apps/{APP_ID}.png"),
            format!("share/icons/hicolor/128x128/apps/{APP_ID}.png"),
            format!("share/icons/hicolor/64x64/apps/{APP_ID}.png"),
            format!("share/icons/hicolor/48x48/apps/{APP_ID}.png"),
            format!("share/icons/hicolor/32x32/apps/{APP_ID}.png"),
            format!("share/metainfo/{APP_ID}.metainfo.xml"),
            "share/man/man1/rusty-wave.1.gz".to_string(),
            format!("share/doc/{PKG}/THIRD_PARTY_LICENSES.md"),
            format!("share/doc/{PKG}/LICENSE-MIT"),
            format!("share/doc/{PKG}/LICENSE-APACHE"),
        ] {
            copy(&stage.join(&rel), &dir.join("usr").join(&rel))?;
        }
        // The files AppImage tooling looks for at the top.
        copy(
            &stage.join(format!("share/applications/{APP_ID}.desktop")),
            &dir.join(format!("{APP_ID}.desktop")),
        )?;
        copy(
            &stage.join(format!("share/icons/hicolor/256x256/apps/{APP_ID}.png")),
            &dir.join(format!("{APP_ID}.png")),
        )?;
        copy(&stage.join(format!("share/icons/hicolor/256x256/apps/{APP_ID}.png")), &dir.join(".DirIcon"))?;
        write(
            &dir.join("AppRun"),
            b"#!/bin/sh\n# Rusty Wave AppImage entry point.\nHERE=\"$(dirname \"$(readlink -f \"$0\")\")\"\nexport XDG_DATA_DIRS=\"$HERE/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}\"\nexec \"$HERE/usr/bin/rusty-wave\" \"$@\"\n",
        )?;
        set_mode(&dir.join("AppRun"), 0o755)?;
        if !have("zsyncmake") {
            return Err("zsyncmake is missing (the AppImage's delta-update file): apt install zsync, dnf install zsync, or build zsync from source".into());
        }
        let out = self.out().join(format!("rusty-wave-{}-x86_64.AppImage", self.version));
        let mut c = Command::new(&tool);
        // The update information goes in before signing, so the embedded signature and the .asc cover the final file. appimagetool
        // also writes a .zsync of its own next to the output when zsyncmake exists; it is replaced below by one that names the versioned file.
        // (run in the scratch folder: that is where appimagetool leaves that file)
        c.current_dir(self.dist().join("appimage"))
            .env("ARCH", "x86_64")
            .env("VERSION", &self.version)
            .arg("--appimage-extract-and-run")
            .arg("--no-appstream")
            .arg("-u")
            .arg(self.update_info());
        if let Some(key) = &self.sign {
            // appimagetool embeds the signature and the public key in the ELF (.sha256_sig, .sig_key); checked by `dist verify`.
            c.args(["--sign", "--sign-key", key]);
        }
        sh(c.arg(&dir).arg(&out))?;
        // The delta-update file: its internal URL is the immutable versioned AppImage (the `latest` alias is a copy of this file), so a
        // zsync client that reads the alias can only ever fetch the file the alias describes.
        let zsync = self.out().join(format!("rusty-wave-{}-x86_64.AppImage.zsync", self.version));
        let versioned_url = format!("{}/rusty-wave-{}-x86_64.AppImage", self.versioned_base(), self.version);
        sh(Command::new("zsyncmake")
            .arg("-u")
            .arg(&versioned_url)
            .arg("-f")
            .arg(out.file_name().unwrap())
            .arg("-o")
            .arg(&zsync)
            .arg(&out))?;
        println!("{}\n{}", out.display(), zsync.display());
        Ok(())
    }

    // ---- Flatpak -----------------------------------------------------------------------------------------------------------------

    fn flatpak_sources(&self) -> Result<(), String> {
        let gen_script = self.root.join("tools/flatpak-cargo-generator.py");
        let venv = self.staging().join("flatpak-venv");
        if !venv.join("bin/python").exists() {
            sh(Command::new("python3").args(["-m", "venv"]).arg(&venv))?;
            sh(Command::new(venv.join("bin/pip"))
                .args(["install", "--quiet", "aiohttp", "tomlkit", "PyYAML"]))?;
        }
        sh(Command::new(venv.join("bin/python"))
            .arg(&gen_script)
            .arg(self.root.join("Cargo.lock"))
            .arg("-o")
            .arg(self.root.join("packaging/flatpak/cargo-sources.json")))
    }

    fn flatpak(&self) -> Result<(), String> {
        if !self.prepare_only && !have("flatpak-builder") {
            return Err("flatpak-builder is missing".into());
        }
        let work = self.staging().join("flatpak");
        fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        let src = self.source_tarball()?;
        let sha = sha256(&src)?;
        // The manifest in the repository builds from a release tag; the local one builds from the tarball of the working tree.
        let manifest = fs::read_to_string(self.root.join(format!("packaging/flatpak/{APP_ID}.yml")))
            .map_err(|e| e.to_string())?;
        let marker = "# @APP-SOURCE@";
        let (head, tail) =
            manifest.split_once(marker).ok_or("the Flatpak manifest has no `# @APP-SOURCE@` marker")?;
        // Everything between the marker and the next `# @END-APP-SOURCE@` is replaced.
        let (_, rest) = tail.split_once("# @END-APP-SOURCE@").ok_or("no `# @END-APP-SOURCE@` marker")?;
        let local = format!(
            "{}      - type: archive\n        path: {}\n        sha256: {sha}\n        strip-components: 1\n{rest}",
            head.trim_end_matches(' '),
            src.display()
        );
        let local = local.replace(
            "cargo-sources.json",
            &self.root.join("packaging/flatpak/cargo-sources.json").to_string_lossy(),
        );
        let mpath = work.join(format!("{APP_ID}.yml"));
        write(&mpath, local.as_bytes())?;
        if self.prepare_only {
            // CI builds it with the flatpak-builder action; this only writes the manifest and the source tarball.
            println!("{}", mpath.display());
            return Ok(());
        }
        let repo = work.join("repo");
        let build = work.join("build");
        let state = work.join("state");
        if repo.join("refs").exists() && have("ostree") {
            // A repo left by an earlier build may hold an unsigned ref on another branch; drop it (best effort).
            let _ = Command::new("ostree")
                .arg(format!("--repo={}", repo.display()))
                .args(["refs", "--delete", &format!("app/{APP_ID}/x86_64/master")])
                .stderr(std::process::Stdio::null())
                .status();
        }
        let mut c = Command::new("flatpak-builder");
        c.env("XDG_DATA_HOME", home().join(".local/share"))
            .env("XDG_CONFIG_HOME", home().join(".config"))
            .env("XDG_CACHE_HOME", home().join(".cache"))
            .args([
                "--user",
                "--install-deps-from=flathub",
                "--force-clean",
                "--disable-rofiles-fuse",
                "--default-branch=stable",
                "--state-dir",
            ])
            .arg(&state)
            .arg("--repo")
            .arg(&repo);
        if let Some(key) = &self.sign {
            c.arg(format!("--gpg-sign={key}"));
        }
        c.arg(&build).arg(&mpath);
        sh(&mut c)?;
        if self.sign.is_some() {
            // Signed summary, appstream branch, static deltas, the default key; then the .flatpakrepo and .flatpakref.
            self.flatpak_publish(&repo)?;
        }
        let bundle = self.out().join(format!("{APP_ID}-{}.flatpak", self.version));
        let mut c = Command::new("flatpak");
        c.env("XDG_DATA_HOME", home().join(".local/share"))
            .env("XDG_CONFIG_HOME", home().join(".config"))
            .env("XDG_CACHE_HOME", home().join(".cache"))
            .arg("build-bundle");
        if self.sign.is_some() {
            // The bundle carries the public key, so installing it verifies the signed commit; the runtime comes from Flathub.
            c.arg(format!(
                "--gpg-keys={}",
                self.root.join("packaging/keys/rusty-wave-release.gpg").display()
            ));
            c.arg("--runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo");
        }
        sh(c.arg(&repo).arg(&bundle).arg(APP_ID).arg("stable"))?;
        println!("{}", bundle.display());
        Ok(())
    }

    // ---- Windows ---------------------------------------------------------------------------------------------------------------

    fn win_stage(&self) -> PathBuf {
        self.dist().join("windows/stage")
    }

    fn windows(&self) -> Result<(), String> {
        let exe = self.dist().join("windows/rusty-wave.exe");
        if !(self.no_build && exe.exists()) {
            let built = if cfg!(windows) {
                sh(self.cargo_cmd().args([
                    "build",
                    "--profile",
                    "dist",
                    "-p",
                    "rvp-host-desktop",
                    "--locked",
                ]))?;
                self.root.join("target/dist/rusty-wave.exe")
            } else {
                self.win_container_build()?;
                self.root.join("target/win/x86_64-pc-windows-gnu/dist/rusty-wave.exe")
            };
            copy(&built, &exe)?;
        }
        let stage = self.win_stage();
        copy(&exe, &stage.join("rusty-wave.exe"))?;
        let license = format!(
            "Rusty Wave is licensed under either of the Apache License, Version 2.0 or the MIT license, at your option.\r\n\r\n===== MIT =====\r\n{}\r\n===== Apache-2.0 =====\r\n{}",
            fs::read_to_string(self.root.join("LICENSE-MIT")).map_err(|e| e.to_string())?,
            fs::read_to_string(self.root.join("LICENSE-APACHE")).map_err(|e| e.to_string())?
        );
        write(&stage.join("LICENSE.txt"), license.as_bytes())?;
        copy(&self.root.join("THIRD_PARTY_LICENSES.md"), &stage.join("THIRD_PARTY_LICENSES.md"))?;
        write(
            &stage.join("README.txt"),
            format!(
                "Rusty Wave {}\r\n\r\nPlays video and music. Run rusty-wave.exe, or use Open with on a media file.\r\nrusty-wave --help lists the options.\r\nSettings and the library index are kept in %APPDATA%\\rusty-wave\\data.\r\nhttps://github.com/unicorntearsproject/rusty-media-player\r\n",
                self.version
            )
            .as_bytes(),
        )?;
        if let Ok(cmd) = std::env::var("RVP_WINDOWS_SIGN_CMD") {
            // Signing hook for the exe itself (osslsigncode or signtool); the installer is signed by Inno Setup's SignTool.
            let line = cmd.replace("$f", &stage.join("rusty-wave.exe").to_string_lossy());
            sh(Command::new("sh").arg("-c").arg(line))?;
        }
        // The portable zip.
        let zip = self.out().join(format!("rusty-wave-{}-windows-x64.zip", self.version));
        zip_dir(&stage, &zip)?;
        println!("{}", zip.display());
        Ok(())
    }

    fn win_container_build(&self) -> Result<(), String> {
        self.ensure_win_image()?;
        sh(podman().args([
            "run",
            "--rm",
            "--userns=keep-id",
            "--user",
            &format!("{}:{}", uid(), gid()),
            "--security-opt",
            "label=disable",
            "-e",
            &format!("HOME={}", home().display()),
            "-e",
            &format!("RVP_BUILD_COMMIT={}", self.build_commit()),
            "-e",
            "RVP_TOOLCHAIN=1.99.0",
            "-v",
            &format!("{0}/.cargo:{0}/.cargo", home().display()),
            "-v",
            &format!("{0}/.rustup:{0}/.rustup", home().display()),
            "-v",
            &format!("{}:/work", self.root.display()),
            "-w",
            "/work",
            WIN_IMAGE,
            "bash",
            "-c",
            &format!(
                "export PATH={}/.cargo/bin:$PATH; rustup target add --toolchain 1.99.0 x86_64-pc-windows-gnu >/dev/null 2>&1; \
                 CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
                 cargo +1.99.0 build --profile dist --target x86_64-pc-windows-gnu -p rvp-host-desktop --locked --target-dir /work/target/win",
                home().display()
            ),
        ]))
    }

    fn ensure_win_image(&self) -> Result<(), String> {
        if image_exists(WIN_IMAGE) {
            return Ok(());
        }
        let base = std::env::var("RVP_FEDORA_IMAGE")
            .unwrap_or_else(|_| "registry.fedoraproject.org/fedora:44".into());
        sh(podman().current_dir(self.root.join("packaging/windows")).args([
            "build",
            "--build-arg",
            &format!("BASE={base}"),
            "-f",
            "Containerfile",
            "-t",
            WIN_IMAGE,
            ".",
        ]))
    }

    fn installer(&self) -> Result<(), String> {
        let stage = self.win_stage();
        if !stage.join("rusty-wave.exe").exists() || !self.no_build {
            self.windows()?;
        }
        let out = self.dist().join("windows");
        if cfg!(windows) {
            let iscc = std::env::var("ISCC").unwrap_or_else(|_| "ISCC.exe".into());
            let mut c = Command::new(iscc);
            c.current_dir(self.root.join("packaging/windows"))
                .arg(format!("/DAppVersion={}", self.version))
                .arg(format!("/DStageDir={}", stage.display()))
                .arg(format!("/DOutDir={}", out.display()));
            if let Ok(cmd) = std::env::var("RVP_WINDOWS_SIGN_CMD") {
                c.arg("/DSign=1").arg(format!("/Srvpsign={cmd}"));
            }
            c.arg("rusty-wave.iss");
            sh(&mut c)?;
        } else {
            self.ensure_win_image()?;
            let prefix = self.staging().join("wineprefix");
            fs::create_dir_all(&prefix).map_err(|e| e.to_string())?;
            let dl = self.dist().join("downloads");
            fs::create_dir_all(&dl).map_err(|e| e.to_string())?;
            let setup = dl.join(format!("innosetup-{INNO_VERSION}.exe"));
            if !setup.exists() {
                sh(Command::new("curl").args(["-fsSL", "-o"]).arg(&setup).arg(INNO_URL))?;
            }
            if sha256(&setup)? != INNO_SHA256 {
                return Err(format!("{} does not match the pinned SHA-256", setup.display()));
            }
            let sign = std::env::var("RVP_WINDOWS_SIGN_CMD").is_ok();
            let script = format!(
                "set -e; export WINEPREFIX=/wp WINEDEBUG=-all WINEARCH=win64; \
                 ISCC='C:\\users\\root\\AppData\\Local\\Programs\\Inno Setup 6\\ISCC.exe'; \
                 if [ ! -e \"/wp/drive_c/users/root/AppData/Local/Programs/Inno Setup 6/ISCC.exe\" ]; then \
                   xvfb-run -a bash -c 'wineboot -u; wine reg add \"HKLM\\Software\\Microsoft\\Windows\\CurrentVersion\" /v ProgramW6432Dir /t REG_SZ /d \"C:\\Program Files\" /f; \
                   wine /dl/innosetup-{INNO_VERSION}.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /CURRENTUSER' >/dev/null 2>&1; fi; \
                 cd /work/packaging/windows; \
                 xvfb-run -a wine \"$ISCC\" /DAppVersion={} /DStageDir=Z:\\\\work\\\\target\\\\dist\\\\windows\\\\stage /DOutDir=Z:\\\\work\\\\target\\\\dist\\\\windows {} rusty-wave.iss",
                self.version,
                if sign { "/DSign=1" } else { "" }
            );
            sh(podman().args([
                "run",
                "--rm",
                "--security-opt",
                "label=disable",
                "-v",
                &format!("{}:/work", self.root.display()),
                "-v",
                &format!("{}:/wp", prefix.display()),
                "-v",
                &format!("{}:/dl", dl.display()),
                WIN_IMAGE,
                "bash",
                "-c",
                &script,
            ]))?;
        }
        let name = format!("rusty-wave-{}-x64-Setup.exe", self.version);
        copy(&out.join(&name), &self.out().join(&name))?;
        println!("{}", self.out().join(&name).display());
        Ok(())
    }

    // ---- the web app -----------------------------------------------------------------------------------------------------------

    fn pwa(&self) -> Result<(), String> {
        if !self.no_build {
            crate::web::build_as(false, true, Some(&self.version))?;
        }
        let zip = self.out().join(format!("{PKG}-web-{}.zip", self.version));
        zip_dir(&self.root.join("target/web"), &zip)?;
        println!("{}", zip.display());
        Ok(())
    }

    // ---- checks, sums ----------------------------------------------------------------------------------------------------------

    fn check(&self) -> Result<(), String> {
        let shared = self.root.join("packaging/shared");
        let tmp = self.dist().join("check");
        fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        if have("desktop-file-validate") {
            sh(Command::new("desktop-file-validate").arg(shared.join(format!("{APP_ID}.desktop"))))?;
        } else {
            println!("(desktop-file-validate is not installed: skipped)");
        }
        let meta = fs::read_to_string(shared.join(format!("{APP_ID}.metainfo.xml.in")))
            .map_err(|e| e.to_string())?;
        let stamped = tmp.join(format!("{APP_ID}.metainfo.xml"));
        write(&stamped, self.stamp_metainfo(&meta).as_bytes())?;
        if have("appstreamcli") {
            // The file is written for current AppStream (1.x: `<developer>`, `vcs-browser`); Ubuntu 22.04 ships 0.15, whose validator
            // does not know them, so its verdict is advisory.
            let version = capture(Command::new("appstreamcli").arg("--version")).unwrap_or_default();
            let major: u32 = version
                .split(|c: char| !c.is_ascii_digit())
                .find(|t| !t.is_empty())
                .and_then(|t| t.parse().ok())
                .unwrap_or(1);
            let result = sh(Command::new("appstreamcli").args(["validate", "--no-net"]).arg(&stamped));
            if let Err(e) = result {
                if major >= 1 {
                    return Err(e);
                }
                println!("(AppStream validator {} is older than 1.0: result is advisory)", version.trim());
            }
        } else {
            println!("(appstreamcli is not installed: skipped)");
        }
        let man = fs::read_to_string(shared.join("rusty-wave.1.in")).map_err(|e| e.to_string())?;
        let man_path = tmp.join("rusty-wave.1");
        write(&man_path, self.stamp(&man).as_bytes())?;
        if have("mandoc") {
            sh(Command::new("mandoc").args(["-T", "lint", "-W", "warning"]).arg(&man_path))?;
        }
        // Every media type of the desktop file is in the AppStream file and the other way round.
        let desktop =
            fs::read_to_string(shared.join(format!("{APP_ID}.desktop"))).map_err(|e| e.to_string())?;
        let types: Vec<&str> = desktop
            .lines()
            .find_map(|l| l.strip_prefix("MimeType="))
            .unwrap_or_default()
            .split(';')
            .filter(|t| !t.is_empty())
            .collect();
        for t in &types {
            if !meta.contains(&format!("<mediatype>{t}</mediatype>")) {
                return Err(format!("`{t}` is in the .desktop file but not in the AppStream <provides>"));
            }
        }
        let flatpak = fs::read_to_string(self.root.join(format!("packaging/flatpak/{APP_ID}.yml")))
            .map_err(|e| e.to_string())?;
        flatpak_permissions_ok(&flatpak)?;
        let iss = fs::read_to_string(self.root.join("packaging/windows/rusty-wave.iss"))
            .map_err(|e| e.to_string())?;
        for ext in ["mp4", "mkv", "webm", "mp3", "flac", "ogg", "opus", "wav", "m4a", "m3u8", "pls"] {
            if !iss.contains(&format!("\"Software\\Classes\\.{ext}\\OpenWithProgids\"")) {
                return Err(format!(".{ext} is not associated in rusty-wave.iss"));
            }
        }
        // The macOS Info.plist: document types agree with the installer's extensions, UTIs are declared, and it is a valid plist.
        let plist =
            fs::read_to_string(self.root.join("packaging/macos/Info.plist.in")).map_err(|e| e.to_string())?;
        macos::check_document_types(&plist, &macos::iss_extensions(&iss))?;
        let stamped_plist = tmp.join("Info.plist");
        write(&stamped_plist, macos::info_plist(&plist, &self.version).as_bytes())?;
        if have("plutil") {
            sh(Command::new("plutil").arg("-lint").arg(&stamped_plist))?;
        } else if have("python3") {
            sh(Command::new("python3")
                .args(["-c", "import plistlib,sys; plistlib.load(open(sys.argv[1],'rb'))"])
                .arg(&stamped_plist))?;
        }
        // Nothing may run on a schedule: workflows start by a tag or by hand only.
        let wf = self.root.join(".github/workflows");
        if let Ok(rd) = fs::read_dir(&wf) {
            for e in rd.filter_map(Result::ok) {
                let text = fs::read_to_string(e.path()).unwrap_or_default();
                if has_schedule(&text) {
                    return Err(format!(
                        "{} has a `schedule:` trigger; workflows must not run on a timer",
                        e.path().display()
                    ));
                }
            }
        }
        flatpak_sources_cover_lockfile(&self.root)?;
        println!("metadata ok ({} media types)", types.len());
        Ok(())
    }

    fn checksums(&self) -> Result<(), String> {
        let out = self.out();
        let mut files: Vec<PathBuf> = fs::read_dir(&out)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.file_name().is_some_and(|n| {
                        let n = n.to_string_lossy();
                        n != "SHA256SUMS"
                            && n != manifest::MANIFEST_NAME
                            && !n.ends_with(".sig")
                            && !n.ends_with(".asc")
                            && !n.ends_with("-src.tar.gz")
                    })
            })
            .collect();
        files.sort();
        let mut sums = String::new();
        for f in &files {
            sums.push_str(&format!("{}  {}\n", sha256(f)?, f.file_name().unwrap().to_string_lossy()));
        }
        write(&out.join("SHA256SUMS"), sums.as_bytes())?;
        if self.sign.is_some() {
            // A detached .asc next to every artifact and the sums (the rpm and AppImage carry embedded signatures as well).
            for f in files.iter().chain(std::iter::once(&out.join("SHA256SUMS"))) {
                self.gpg_detach(f)?;
            }
        }
        if let Ok(cmd) = std::env::var("RVP_SIGN_CMD") {
            // Signing hook: run once per artifact and for the sums, `{}` is the path (for example `gpg --batch --yes --armor --detach-sign {}`).
            for f in files.iter().chain(std::iter::once(&out.join("SHA256SUMS"))) {
                sh(Command::new("sh").arg("-c").arg(cmd.replace("{}", &f.to_string_lossy())))?;
            }
        }
        print!("{sums}");
        Ok(())
    }
}

/// Every registry crate in Cargo.lock has an entry in `packaging/flatpak/cargo-sources.json` (the Flatpak builds offline from it): a new
/// dependency without `cargo xtask dist flatpak-sources` breaks only the CI Flatpak job, so `dist check` catches it first.
fn flatpak_sources_cover_lockfile(root: &Path) -> Result<(), String> {
    let lock = fs::read_to_string(root.join("Cargo.lock")).map_err(|e| e.to_string())?;
    let sources =
        fs::read_to_string(root.join("packaging/flatpak/cargo-sources.json")).map_err(|e| e.to_string())?;
    let mut missing = Vec::new();
    for block in lock.split("[[package]]").skip(1) {
        let field = |k: &str| {
            block
                .lines()
                .find_map(|l| l.strip_prefix(k)?.strip_prefix(" = \"")?.strip_suffix('"'))
                .map(str::to_string)
        };
        let (Some(name), Some(version), Some(source)) = (field("name"), field("version"), field("source"))
        else {
            continue;
        };
        if source.starts_with("registry+") && !sources.contains(&format!("\"cargo/vendor/{name}-{version}\""))
        {
            missing.push(format!("{name} {version}"));
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "packaging/flatpak/cargo-sources.json is out of date, missing: {}; run `cargo xtask dist flatpak-sources`",
            missing.join(", ")
        ))
    }
}

/// Whether a workflow file has a `schedule:` key (a cron trigger) outside comments.
fn has_schedule(text: &str) -> bool {
    text.lines().any(|l| {
        let l = l.trim_start();
        !l.starts_with('#') && (l.starts_with("schedule:") || l.starts_with("- cron:"))
    })
}

fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|e| e.to_string())
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

fn uid() -> String {
    capture(Command::new("id").arg("-u")).unwrap_or_else(|_| "1000".into()).trim().to_string()
}

fn gid() -> String {
    capture(Command::new("id").arg("-g")).unwrap_or_else(|_| "1000".into()).trim().to_string()
}

/// Zip the contents of `dir` (not the directory itself) into `zip`, with the `zip` tool or Python's `zipfile`.
fn zip_dir(dir: &Path, zip: &Path) -> Result<(), String> {
    if zip.exists() {
        fs::remove_file(zip).map_err(|e| e.to_string())?;
    }
    if have("zip") {
        return sh(Command::new("zip").current_dir(dir).args(["-q", "-r", "-X"]).arg(zip).arg("."));
    }
    let py = "import sys, os, zipfile\nsrc, out = sys.argv[1], sys.argv[2]\nz = zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED)\nfor base, _, files in os.walk(src):\n    for f in sorted(files):\n        p = os.path.join(base, f)\n        z.write(p, os.path.relpath(p, src))\nz.close()\n";
    let python = if have("python3") { "python3" } else { "python" };
    sh(Command::new(python).arg("-c").arg(py).arg(dir).arg(zip))
}

#[cfg(test)]
mod tests {
    use super::has_schedule;

    #[test]
    fn schedules_are_found() {
        assert!(has_schedule("on:\n  schedule:\n    - cron: '0 3 * * *'\n"));
        assert!(!has_schedule("on:\n  push:\n    tags: ['v*']\n# schedule: never\n"));
    }
}

#[cfg(test)]
mod version_tests {
    use super::*;

    #[test]
    fn the_flatpak_sources_cover_every_locked_crate() {
        flatpak_sources_cover_lockfile(&Path::new(env!("CARGO_MANIFEST_DIR")).join("..")).unwrap();
    }

    #[test]
    fn a_release_candidate_is_spelled_the_way_each_package_system_orders_it() {
        assert_eq!(rpm_parts("1.0.0-rc1"), ("1.0.0".to_string(), "0.1.rc1".to_string()));
        assert_eq!(rpm_parts("1.0.0-rc.2"), ("1.0.0".to_string(), "0.1.rc.2".to_string()));
        assert_eq!(rpm_parts("1.0.0"), ("1.0.0".to_string(), "1".to_string()));
        // rc10 would sort below rc9 as text, so the counter after rc9 is rc9.1, rc9.2 ...: rpm compares the extra field as newer.
        assert_eq!(rpm_parts("1.0.0-rc9.1"), ("1.0.0".to_string(), "0.1.rc9.1".to_string()));
        assert_eq!(rpm_parts("0.0.0-ci1"), ("0.0.0".to_string(), "0.1.ci1".to_string()));
        // rpm compares the release field piece by piece: 0.1.rc1 is older than 1 (the release).
        let key = |r: &str| r.split('.').map(|p| p.parse::<u32>().ok()).collect::<Vec<_>>();
        assert!(key("0.1.rc1") < key("1") || key("0.1.rc1")[0] < key("1")[0]);
    }

    #[test]
    fn the_flatpak_must_grant_what_tags_themes_and_defaults_need() {
        let ok = "finish-args:\n  - --filesystem=xdg-music\n  - --filesystem=xdg-videos\n  - --share=network\n  - --filesystem=xdg-config/mimeapps.list\n";
        assert!(flatpak_permissions_ok(ok).is_ok());
        for gone in ["--share=network", "--filesystem=xdg-config/mimeapps.list", "--filesystem=xdg-music"] {
            let bad = ok.replace(&format!("  - {gone}\n"), "");
            assert!(
                flatpak_permissions_ok(&bad).unwrap_err().contains(gone.trim_start_matches("--")),
                "{gone}"
            );
        }
        let ro = ok.replace("xdg-videos", "xdg-videos:ro");
        assert!(flatpak_permissions_ok(&ro).is_err());
        let real = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../packaging/flatpak/io.github.unicorntearsproject.RustyWave.yml"),
        )
        .unwrap();
        assert!(flatpak_permissions_ok(&real).is_ok());
    }
}
