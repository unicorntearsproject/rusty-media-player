//! The whole update path against a throwaway release directory signed with a throwaway key.
use crate::apply::{self, Finish};
use crate::fetch::{Cancel, Net, sha256_hex};
use crate::kind::InstallKind;
use crate::updater::{Checked, Config, Offer, State, Updater, check, install};
use crate::verify::{Verifier, testkit};
use crate::{UpdateError, VerifyError, Version};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("rvp-update-flow-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A release directory: files, their signatures and a signed manifest.
struct Release {
    dir: PathBuf,
    key: testkit::TestKey,
}

impl Release {
    fn new(name: &str, version: &str, files: &[(&str, &str, &[u8])]) -> Release {
        let dir = tmp(name);
        let key = testkit::key("Release <r@example.com>");
        let r = Release { dir, key };
        r.write(version, files, &testkit::fingerprint(&r.key));
        r
    }

    /// Write files, signatures and the manifest (naming `fingerprint` as the signing key).
    fn write(&self, version: &str, files: &[(&str, &str, &[u8])], fingerprint: &str) {
        let mut entries = Vec::new();
        for (key, name, data) in files {
            std::fs::write(self.dir.join(name), data).unwrap();
            std::fs::write(self.dir.join(format!("{name}.asc")), testkit::sign(&self.key, data)).unwrap();
            entries.push(format!(
                r#""{key}": {{"arch":"x","name":"{name}","url":"{d}/{name}","size":{len},"sha256":"{sha}","signature_url":"{d}/{name}.asc"}}"#,
                d = self.dir.display(),
                len = data.len(),
                sha = sha256_hex(data)
            ));
        }
        let manifest = format!(
            r#"{{"schema":1,"version":"{version}","released":"2026-10-06","key_fingerprint":"{fingerprint}","files":{{{}}}}}"#,
            entries.join(",")
        );
        self.sign_manifest(manifest.as_bytes());
    }

    fn sign_manifest(&self, manifest: &[u8]) {
        std::fs::write(self.dir.join("latest.json"), manifest).unwrap();
        std::fs::write(self.dir.join("latest.json.asc"), testkit::sign(&self.key, manifest)).unwrap();
    }

    fn manifest(&self) -> String {
        self.dir.join("latest.json").to_string_lossy().into_owned()
    }

    fn config(&self, current: &str, kind: InstallKind, work: &Path) -> Config {
        Config {
            manifest_url: self.manifest(),
            current: Version::parse(current).unwrap(),
            kind,
            verifier: Arc::new(Verifier::from_armored(&self.key.public_armored).unwrap()),
            source: Arc::new(Net::new("test")),
            temp_dir: work.join("tmp"),
            restart_args: Vec::new(),
        }
    }
}

fn appimage_release(name: &str, version: &str, data: &[u8]) -> Release {
    Release::new(name, version, &[("linux-appimage", "rusty-wave-new.AppImage", data)])
}

fn installed_appimage(dir: &Path, content: &[u8]) -> PathBuf {
    let p = dir.join("Rusty Wave.AppImage");
    std::fs::write(&p, content).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o750)).unwrap();
    }
    p
}

fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".part") || n.ends_with(".new") || n.ends_with(".old"))
        .collect()
}

fn offer_file(c: Checked) -> crate::FileEntry {
    match c {
        Checked::Newer(Offer::Update { file, .. }) => file,
        other => panic!("expected an update, got {other:?}"),
    }
}

// ---- checking -----------------------------------------------------------------------------------------------------------------

#[test]
fn newer_is_offered_equal_older_and_replayed_are_not() {
    let work = tmp("check1");
    let app = installed_appimage(&work, b"old");
    let r = appimage_release("check1-rel", "0.0.3", b"new");
    let cfg = r.config("0.0.2", InstallKind::AppImage(app.clone()), &work);
    let Checked::Newer(Offer::Update { release, file }) = check(&cfg).unwrap() else { panic!() };
    assert_eq!(release.version.to_string(), "0.0.3");
    assert_eq!(file.name, "rusty-wave-new.AppImage");
    // Same version, and a validly signed manifest older than what runs (a replay): nothing to offer, never a downgrade.
    for current in ["0.0.3", "0.0.4", "1.0.0"] {
        let cfg = r.config(current, InstallKind::AppImage(app.clone()), &work);
        assert!(matches!(check(&cfg).unwrap(), Checked::UpToDate(_)), "{current}");
    }
    // A pre-release build is older than its release.
    let cfg = r.config("0.0.3-ci1", InstallKind::AppImage(app), &work);
    assert!(matches!(check(&cfg).unwrap(), Checked::Newer(_)));
}

#[test]
fn a_manifest_that_is_not_signed_by_the_release_key_is_refused() {
    let work = tmp("check2");
    let app = installed_appimage(&work, b"old");
    let r = appimage_release("check2-rel", "0.0.3", b"new");
    let cfg = r.config("0.0.2", InstallKind::AppImage(app), &work);
    // Changed after signing.
    let original = std::fs::read(r.dir.join("latest.json")).unwrap();
    let mut tampered = original.clone();
    tampered.extend_from_slice(b" ");
    std::fs::write(r.dir.join("latest.json"), &tampered).unwrap();
    assert_eq!(check(&cfg), Err(UpdateError::Signature(VerifyError::Mismatch)));
    // Signed by somebody else.
    std::fs::write(r.dir.join("latest.json"), &original).unwrap();
    let other = testkit::key("Mallory <m@example.com>");
    std::fs::write(r.dir.join("latest.json.asc"), testkit::sign(&other, &original)).unwrap();
    assert_eq!(check(&cfg), Err(UpdateError::Signature(VerifyError::WrongKey)));
    // No signature at all.
    std::fs::remove_file(r.dir.join("latest.json.asc")).unwrap();
    assert!(matches!(check(&cfg), Err(UpdateError::Io(_))));
}

#[test]
fn a_manifest_naming_another_key_or_a_future_schema_is_refused() {
    let work = tmp("check3");
    let app = installed_appimage(&work, b"old");
    let r = appimage_release("check3-rel", "0.0.3", b"new");
    let cfg = r.config("0.0.2", InstallKind::AppImage(app), &work);
    let good = std::fs::read_to_string(r.dir.join("latest.json")).unwrap();
    let fp = testkit::fingerprint(&r.key);
    r.sign_manifest(good.replace(&fp, "0000000000000000000000000000000000000000").as_bytes());
    assert_eq!(check(&cfg), Err(UpdateError::UntrustedKey));
    r.sign_manifest(good.replace("\"schema\":1", "\"schema\":2").as_bytes());
    assert_eq!(check(&cfg), Err(UpdateError::UnsupportedSchema(2)));
    r.sign_manifest(good.replace("0.0.3", "next").as_bytes());
    assert!(matches!(check(&cfg), Err(UpdateError::BadManifest(_))));
    r.sign_manifest(good.replace("rusty-wave-new.AppImage\",\"url", "../evil\",\"url").as_bytes());
    assert!(matches!(check(&cfg), Err(UpdateError::BadManifest(_))));
}

#[test]
fn installs_that_a_package_manager_owns_get_a_message_not_an_update() {
    let work = tmp("check4");
    let r = Release::new(
        "check4-rel",
        "0.0.3",
        &[("linux-deb", "x.deb", b"d"), ("macos-dmg", "x.dmg", b"m"), ("linux-appimage", "x.AppImage", b"a")],
    );
    for (kind, text) in [
        (InstallKind::Package, "package manager"),
        (InstallKind::Flatpak, "Flatpak"),
        (InstallKind::Other, "site"),
    ] {
        let cfg = r.config("0.0.2", kind, &work);
        let Checked::Newer(Offer::Manual { message, url, release }) = check(&cfg).unwrap() else { panic!() };
        assert!(message.contains(text), "{message}");
        assert_eq!(url, None);
        assert_eq!(release.version.to_string(), "0.0.3");
    }
    let cfg = r.config("0.0.2", InstallKind::MacApp, &work);
    let Checked::Newer(Offer::Manual { url, .. }) = check(&cfg).unwrap() else { panic!() };
    assert!(url.unwrap().ends_with("x.dmg"));
    // An AppImage whose manifest has no file for it is told to download by hand rather than failing.
    let r2 = Release::new("check4-rel2", "0.0.3", &[("linux-deb", "x.deb", b"d")]);
    let cfg = r2.config("0.0.2", InstallKind::AppImage(work.join("a")), &work);
    assert!(matches!(check(&cfg).unwrap(), Checked::Newer(Offer::Manual { .. })));
}

// ---- installing: AppImage --------------------------------------------------------------------------------------------------

#[test]
fn the_appimage_is_replaced_in_one_step_keeping_its_mode() {
    let work = tmp("inst1");
    let app = installed_appimage(&work, b"old appimage");
    let r = appimage_release("inst1-rel", "0.0.3", b"new appimage");
    let cfg = r.config("0.0.2", InstallKind::AppImage(app.clone()), &work);
    let file = offer_file(check(&cfg).unwrap());
    let mut seen = Vec::new();
    let f = install(&cfg, &file, &Cancel::new(), &mut |d, t| seen.push((d, t))).unwrap();
    assert_eq!(f, Finish::Restart { exe: app.clone() });
    assert_eq!(std::fs::read(&app).unwrap(), b"new appimage");
    assert!(leftovers(&work).is_empty(), "{:?}", leftovers(&work));
    assert_eq!(seen.last(), Some(&(12, 12)));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&app).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o751, "the old file's mode plus execute for all");
    }
}

/// A source standing in for a CDN edge that ignores everything but "give me the whole file" (a cold CloudFront edge over S3 may answer a
/// zsync multi-range request with a 200 and the full body): it records what was fetched.
struct WholeFilesOnly {
    inner: Net,
    opened: std::sync::Mutex<Vec<String>>,
}

impl crate::fetch::Source for WholeFilesOnly {
    fn open(&self, url: &str) -> Result<(Box<dyn std::io::Read + Send>, Option<u64>), UpdateError> {
        self.opened.lock().unwrap().push(url.to_string());
        self.inner.open(url)
    }
}

#[test]
fn the_updater_downloads_whole_files_so_a_server_that_ignores_ranges_changes_nothing() {
    // The built-in updater never asks for a byte range (the zsync delta is for AppImageUpdate and other tools): it fetches the whole file
    // and checks its size, SHA-256 and signature before touching anything. A server that answers every request with a 200 and the full
    // body is therefore the normal case, and a download that arrives damaged is refused.
    let work = tmp("whole");
    let app = installed_appimage(&work, b"old appimage");
    let r = appimage_release("whole-rel", "0.0.3", b"new appimage");
    let src = Arc::new(WholeFilesOnly { inner: Net::new("test"), opened: Default::default() });
    let mut cfg = r.config("0.0.2", InstallKind::AppImage(app.clone()), &work);
    cfg.source = src.clone();
    let file = offer_file(check(&cfg).unwrap());
    install(&cfg, &file, &Cancel::new(), &mut |_, _| {}).unwrap();
    assert_eq!(std::fs::read(&app).unwrap(), b"new appimage");
    let opened = src.opened.lock().unwrap().clone();
    assert_eq!(opened.iter().filter(|u| u.ends_with("rusty-wave-new.AppImage")).count(), 1, "{opened:?}");
    // The same release with the file damaged in transit (right length, wrong bytes): refused, the installed file untouched.
    let work2 = tmp("whole2");
    let app2 = installed_appimage(&work2, b"old appimage");
    let r2 = appimage_release("whole-rel2", "0.0.3", b"new appimage");
    let mut cfg2 = r2.config("0.0.2", InstallKind::AppImage(app2.clone()), &work2);
    cfg2.source = Arc::new(WholeFilesOnly { inner: Net::new("test"), opened: Default::default() });
    let file2 = offer_file(check(&cfg2).unwrap());
    std::fs::write(r2.dir.join("rusty-wave-new.AppImage"), b"NEW appimage").unwrap();
    assert!(install(&cfg2, &file2, &Cancel::new(), &mut |_, _| {}).is_err());
    assert_eq!(std::fs::read(&app2).unwrap(), b"old appimage");
}

#[test]
fn a_damaged_or_forged_download_changes_nothing() {
    let work = tmp("inst2");
    let app = installed_appimage(&work, b"old appimage");
    let r = appimage_release("inst2-rel", "0.0.3", b"new appimage");
    let cfg = r.config("0.0.2", InstallKind::AppImage(app.clone()), &work);
    let file = offer_file(check(&cfg).unwrap());
    let path = r.dir.join(&file.name);
    let untouched = |why: &str| {
        assert_eq!(std::fs::read(&app).unwrap(), b"old appimage", "{why}");
        assert!(leftovers(&work).is_empty(), "{why}: {:?}", leftovers(&work));
    };

    // The bytes changed, same length: the checksum catches it.
    std::fs::write(&path, b"new appimagE").unwrap();
    assert!(matches!(install(&cfg, &file, &Cancel::new(), &mut |_, _| {}), Err(UpdateError::Corrupt(_))));
    untouched("changed bytes");
    // Truncated.
    std::fs::write(&path, b"new app").unwrap();
    assert!(matches!(install(&cfg, &file, &Cancel::new(), &mut |_, _| {}), Err(UpdateError::Corrupt(_))));
    untouched("truncated");
    // Longer than announced.
    std::fs::write(&path, b"new appimage plus a tail").unwrap();
    assert!(matches!(install(&cfg, &file, &Cancel::new(), &mut |_, _| {}), Err(UpdateError::Corrupt(_))));
    untouched("longer");
    // Right bytes but the signature is another key's (a compromised mirror with a matching manifest entry).
    std::fs::write(&path, b"new appimage").unwrap();
    let mallory = testkit::key("Mallory <m@example.com>");
    std::fs::write(r.dir.join(format!("{}.asc", file.name)), testkit::sign(&mallory, b"new appimage"))
        .unwrap();
    assert_eq!(
        install(&cfg, &file, &Cancel::new(), &mut |_, _| {}),
        Err(UpdateError::Signature(VerifyError::WrongKey))
    );
    untouched("wrong key");
    // A signature over other bytes.
    std::fs::write(r.dir.join(format!("{}.asc", file.name)), testkit::sign(&r.key, b"something else"))
        .unwrap();
    assert_eq!(
        install(&cfg, &file, &Cancel::new(), &mut |_, _| {}),
        Err(UpdateError::Signature(VerifyError::Mismatch))
    );
    untouched("signature of other bytes");
    // Missing signature file.
    std::fs::remove_file(r.dir.join(format!("{}.asc", file.name))).unwrap();
    assert!(matches!(install(&cfg, &file, &Cancel::new(), &mut |_, _| {}), Err(UpdateError::Io(_))));
    untouched("no signature");
}

#[test]
fn cancel_stops_and_leaves_the_installed_file_alone() {
    let work = tmp("inst3");
    let app = installed_appimage(&work, b"old");
    let big = vec![7u8; 600_000];
    let r = appimage_release("inst3-rel", "0.0.3", &big);
    let cfg = r.config("0.0.2", InstallKind::AppImage(app.clone()), &work);
    let file = offer_file(check(&cfg).unwrap());
    let cancel = Cancel::new();
    let c2 = cancel.clone();
    let res = install(&cfg, &file, &cancel, &mut |done, _| {
        if done > 100_000 {
            c2.cancel();
        }
    });
    assert_eq!(res, Err(UpdateError::Cancelled));
    assert_eq!(std::fs::read(&app).unwrap(), b"old");
    assert!(leftovers(&work).is_empty());
}

#[cfg(unix)]
#[test]
fn a_folder_without_write_permission_is_reported_in_words() {
    use std::os::unix::fs::PermissionsExt;
    let work = tmp("inst4");
    let app = installed_appimage(&work, b"old");
    let r = appimage_release("inst4-rel", "0.0.3", b"new");
    let cfg = r.config("0.0.2", InstallKind::AppImage(app.clone()), &work);
    let file = offer_file(check(&cfg).unwrap());
    std::fs::set_permissions(&work, std::fs::Permissions::from_mode(0o555)).unwrap();
    let probe = std::fs::File::create(work.join(".probe"));
    let res = install(&cfg, &file, &Cancel::new(), &mut |_, _| {});
    std::fs::set_permissions(&work, std::fs::Permissions::from_mode(0o755)).unwrap();
    if probe.is_ok() {
        return; // running as root: permissions do not apply
    }
    assert!(matches!(res, Err(UpdateError::NotWritable(_))), "{res:?}");
    assert_eq!(std::fs::read(&app).unwrap(), b"old");
}

// ---- installing: Windows -----------------------------------------------------------------------------------------------------

fn zip_with(exe: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        z.start_file("rusty-wave/README.md", o).unwrap();
        z.write_all(b"readme").unwrap();
        z.start_file("rusty-wave/rusty-wave.exe", o).unwrap();
        z.write_all(exe).unwrap();
        z.finish().unwrap();
    }
    buf.into_inner()
}

#[test]
fn the_installer_is_downloaded_verified_and_left_for_the_caller_to_run() {
    let work = tmp("win1");
    let r = Release::new(
        "win1-rel",
        "0.0.3",
        &[("windows-installer", "rusty-wave-0.0.3-x64-Setup.exe", b"MZ setup")],
    );
    let cfg = r.config("0.0.2", InstallKind::WindowsInstaller, &work);
    let file = offer_file(check(&cfg).unwrap());
    let Finish::RunInstaller { path } = install(&cfg, &file, &Cancel::new(), &mut |_, _| {}).unwrap() else {
        panic!()
    };
    assert_eq!(std::fs::read(&path).unwrap(), b"MZ setup");
    assert!(path.starts_with(work.join("tmp")));
    // A bad one is deleted, not kept around to be run by mistake.
    std::fs::write(r.dir.join("rusty-wave-0.0.3-x64-Setup.exe"), b"MZ evil!").unwrap();
    assert!(install(&cfg, &file, &Cancel::new(), &mut |_, _| {}).is_err());
    assert!(!path.exists());
}

#[test]
fn the_portable_exe_is_taken_from_the_zip_and_swapped_in_by_rename() {
    let work = tmp("win2");
    let current = work.join("rusty-wave.exe");
    std::fs::write(&current, b"#!/bin/sh\nexit 3\n").unwrap();
    let new_exe = b"#!/bin/sh\nexit 0\n";
    let zip = zip_with(new_exe);
    let r =
        Release::new("win2-rel", "0.0.3", &[("windows-portable", "rusty-wave-0.0.3-windows-x64.zip", &zip)]);
    let cfg = r.config("0.0.2", InstallKind::WindowsPortable(current.clone()), &work);
    let file = offer_file(check(&cfg).unwrap());
    let finish = install(&cfg, &file, &Cancel::new(), &mut |_, _| {}).unwrap();
    let new = work.join("rusty-wave.exe.new");
    assert_eq!(finish, Finish::SwapExe { new: new.clone(), current: current.clone() });
    assert_eq!(std::fs::read(&new).unwrap(), new_exe);
    assert_eq!(
        std::fs::read(&current).unwrap(),
        b"#!/bin/sh\nexit 3\n",
        "nothing is swapped before the restart"
    );
    assert!(!work.join("tmp").join("rusty-wave-0.0.3-windows-x64.zip").exists(), "the zip is not kept");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755)).unwrap();
        apply::run_finish(&finish, &[]).unwrap();
        assert_eq!(std::fs::read(&current).unwrap(), new_exe);
        assert_eq!(std::fs::read(work.join("rusty-wave.exe.old")).unwrap(), b"#!/bin/sh\nexit 3\n");
        assert!(!new.exists());
        // The next start removes the .old file.
        apply::clean_leftovers(&InstallKind::WindowsPortable(current.clone()));
        assert!(!work.join("rusty-wave.exe.old").exists());
    }
}

#[cfg(unix)]
#[test]
fn a_swap_that_cannot_start_the_new_program_puts_the_old_one_back() {
    let work = tmp("win3");
    let current = work.join("rusty-wave.exe");
    std::fs::write(&current, b"old").unwrap();
    let new = work.join("rusty-wave.exe.new");
    std::fs::write(&new, b"not executable").unwrap(); // spawning it fails
    let r = apply::run_finish(&Finish::SwapExe { new, current: current.clone() }, &[]);
    assert!(matches!(r, Err(UpdateError::Io(_))), "{r:?}");
    assert_eq!(std::fs::read(&current).unwrap(), b"old");
    assert!(!work.join("rusty-wave.exe.old").exists());
}

#[test]
fn a_zip_without_the_program_or_a_broken_zip_is_refused() {
    let work = tmp("win4");
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        use std::io::Write;
        let mut z = zip::ZipWriter::new(&mut buf);
        z.start_file("readme.txt", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(b"x").unwrap();
        z.finish().unwrap();
    }
    std::fs::write(work.join("a.zip"), buf.into_inner()).unwrap();
    assert!(matches!(
        apply::extract_exe(&work.join("a.zip"), &work.join("out")),
        Err(UpdateError::Corrupt(_))
    ));
    std::fs::write(work.join("b.zip"), b"PK not really").unwrap();
    assert!(matches!(
        apply::extract_exe(&work.join("b.zip"), &work.join("out")),
        Err(UpdateError::Corrupt(_))
    ));
}

// ---- the service -------------------------------------------------------------------------------------------------------------

fn wait_for(u: &Updater, what: &str, f: impl Fn(&State) -> bool) -> State {
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        let s = u.state();
        if f(&s) {
            return s;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}; state {s:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_service_goes_from_checking_to_ready() {
    let work = tmp("svc1");
    let app = installed_appimage(&work, b"old");
    let data = vec![9u8; 2_000_000];
    let r = appimage_release("svc1-rel", "0.0.3", &data);
    let u = Updater::new(r.config("0.0.2", InstallKind::AppImage(app.clone()), &work));
    assert_eq!(u.state(), State::Idle);
    u.install(); // nothing offered yet: ignored
    assert_eq!(u.state(), State::Idle);
    u.check();
    let s = wait_for(&u, "an offer", |s| matches!(s, State::Available(_)));
    assert!(matches!(s, State::Available(Offer::Update { .. })));
    u.install();
    let s = wait_for(&u, "ready", |s| matches!(s, State::Ready(_)));
    let State::Ready(rel) = s else { panic!() };
    assert_eq!(rel.version.to_string(), "0.0.3");
    assert_eq!(std::fs::read(&app).unwrap(), data);
}

#[test]
fn the_service_reports_failures_in_words_and_a_second_check_after_a_fix_works() {
    let work = tmp("svc2");
    let app = installed_appimage(&work, b"old");
    let r = appimage_release("svc2-rel", "0.0.3", b"new");
    let u = Updater::new(r.config("0.0.2", InstallKind::AppImage(app), &work));
    let manifest = std::fs::read(r.dir.join("latest.json")).unwrap();
    std::fs::write(r.dir.join("latest.json"), b"{}").unwrap();
    u.check();
    let State::Failed(msg) = wait_for(&u, "a failure", |s| matches!(s, State::Failed(_))) else { panic!() };
    assert!(msg.starts_with("Could not check for updates"), "{msg}");
    std::fs::write(r.dir.join("latest.json"), manifest).unwrap();
    u.check();
    wait_for(&u, "an offer", |s| matches!(s, State::Available(_)));
    // Up to date.
    let u2 = Updater::new(r.config("0.0.3", InstallKind::AppImage(work.join("x")), &work));
    u2.check();
    wait_for(&u2, "up to date", |s| matches!(s, State::UpToDate(_)));
}

#[test]
fn restart_without_an_installed_update_is_an_error() {
    let work = tmp("svc3");
    let r = appimage_release("svc3-rel", "0.0.3", b"new");
    let u = Updater::new(r.config("0.0.2", InstallKind::AppImage(work.join("a")), &work));
    assert!(u.restart().is_err());
}
