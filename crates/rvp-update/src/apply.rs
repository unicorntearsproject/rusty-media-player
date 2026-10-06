//! Downloading a release, verifying it and putting it in place.
//!
//! Nothing is replaced until the file has the exact size and SHA-256 of the manifest *and* a detached OpenPGP signature over it
//! verifies with the release key. A failure at any step removes what was downloaded and leaves the installed program untouched.
use crate::error::UpdateError;
use crate::fetch::{Cancel, Source, download_to, fetch_bytes};
use crate::kind::InstallKind;
use crate::manifest::FileEntry;
use crate::verify::Verifier;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Signatures are tiny; anything bigger is not one.
const MAX_SIGNATURE: u64 = 64 * 1024;
/// The portable program inside its zip (a sanity cap, in bytes).
const MAX_EXE: u64 = 512 * 1024 * 1024;
/// What the program's executable is called inside the portable zip.
const EXE_NAME: &str = "rusty-wave.exe";

/// What is left to do once an update is in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finish {
    /// Start the new AppImage and quit.
    Restart {
        /// The AppImage that was replaced.
        exe: PathBuf,
    },
    /// Run the Setup program silently and quit.
    RunInstaller {
        /// The verified Setup program.
        path: PathBuf,
    },
    /// Rename the running executable aside, put the new one in its place, start it and quit.
    SwapExe {
        /// The verified new executable.
        new: PathBuf,
        /// The running executable.
        current: PathBuf,
    },
}

/// Everything an install needs.
pub struct Job<'a> {
    /// Where bytes come from.
    pub source: &'a dyn Source,
    /// The key to trust.
    pub verifier: &'a Verifier,
    /// How this copy is installed.
    pub kind: &'a InstallKind,
    /// Where Setup programs and zips are downloaded to.
    pub temp_dir: &'a Path,
    /// The file to install.
    pub entry: &'a FileEntry,
    /// Set by the user's Cancel.
    pub cancel: &'a Cancel,
}

/// A name from a manifest must be a plain file name.
pub(crate) fn safe_name(name: &str) -> Result<&str, UpdateError> {
    if name.is_empty() || name.contains(['/', '\\', '\0']) || name == "." || name == ".." {
        return Err(UpdateError::BadManifest(format!("`{name}` is not a file name")));
    }
    Ok(name)
}

/// Download `entry` to `dest` and check its size, SHA-256 and signature. Removes `dest` on any failure.
pub fn fetch_verified(
    source: &dyn Source,
    verifier: &Verifier,
    entry: &FileEntry,
    dest: &Path,
    cancel: &Cancel,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(), UpdateError> {
    let r = (|| {
        let got =
            download_to(source, &entry.url, dest, entry.size, cancel, progress).map_err(|e| match e {
                UpdateError::TooLarge => UpdateError::Corrupt("it is larger than announced".into()),
                e => e,
            })?;
        if got.size != entry.size {
            return Err(UpdateError::Corrupt(format!("{} bytes instead of {}", got.size, entry.size)));
        }
        if !got.sha256.eq_ignore_ascii_case(entry.sha256.trim()) {
            return Err(UpdateError::Corrupt("its SHA-256 differs from the published one".into()));
        }
        let sig = fetch_bytes(source, &entry.signature_url, MAX_SIGNATURE)?;
        let data = std::fs::read(dest)?;
        verifier.verify_detached(&data, &sig)?;
        Ok(())
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    r
}

/// Download, verify and put the release in place for this kind of install.
pub fn install(job: &Job<'_>, progress: &mut dyn FnMut(u64, u64)) -> Result<Finish, UpdateError> {
    let name = safe_name(&job.entry.name)?;
    match job.kind {
        InstallKind::AppImage(target) => install_appimage(job, target, progress),
        InstallKind::WindowsInstaller => {
            std::fs::create_dir_all(job.temp_dir)?;
            let path = job.temp_dir.join(name);
            fetch_verified(job.source, job.verifier, job.entry, &path, job.cancel, progress)?;
            Ok(Finish::RunInstaller { path })
        }
        InstallKind::WindowsPortable(current) => {
            std::fs::create_dir_all(job.temp_dir)?;
            let zip = job.temp_dir.join(name);
            fetch_verified(job.source, job.verifier, job.entry, &zip, job.cancel, progress)?;
            let new = sibling(current, ".new");
            let r = extract_exe(&zip, &new);
            let _ = std::fs::remove_file(&zip);
            r.map_err(|e| match e {
                UpdateError::Io(m) if m.contains("denied") => {
                    UpdateError::NotWritable(parent_display(current))
                }
                e => e,
            })?;
            Ok(Finish::SwapExe { new, current: current.clone() })
        }
        _ => Err(UpdateError::NotSelfUpdatable),
    }
}

fn parent_display(p: &Path) -> String {
    p.parent().map_or_else(|| p.display().to_string(), |d| d.display().to_string())
}

/// `<path><suffix>` next to `path`.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut s: OsString = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn install_appimage(
    job: &Job<'_>,
    target: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Finish, UpdateError> {
    let dir = target.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let stem = target.file_name().map_or_else(|| "rusty-wave".into(), |n| n.to_string_lossy().into_owned());
    let tmp = dir.join(format!(".{stem}.update-{}.part", std::process::id()));
    // Fail early, and in words, if the folder cannot be written (the usual reason is /opt or a system folder).
    match std::fs::File::create(&tmp) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(30) => {
            return Err(UpdateError::NotWritable(dir.display().to_string()));
        }
        Err(e) => return Err(e.into()),
    }
    fetch_verified(job.source, job.verifier, job.entry, &tmp, job.cancel, progress)?;
    let r: Result<(), UpdateError> = (|| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(target).map(|m| m.permissions().mode()).unwrap_or(0o755);
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode((mode | 0o111) & 0o7777))?;
        }
        // One rename: the old file is replaced in a single step, also while it is running.
        std::fs::rename(&tmp, target)?;
        Ok(())
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r?;
    Ok(Finish::Restart { exe: target.to_path_buf() })
}

/// Write `rusty-wave.exe` out of the portable zip to `out`.
pub fn extract_exe(zip_path: &Path, out: &Path) -> Result<(), UpdateError> {
    let f = std::fs::File::open(zip_path)?;
    let mut z =
        zip::ZipArchive::new(f).map_err(|e| UpdateError::Corrupt(format!("the zip is unreadable: {e}")))?;
    let mut found = None;
    for i in 0..z.len() {
        let entry = z.by_index(i).map_err(|e| UpdateError::Corrupt(e.to_string()))?;
        let base = entry.name().rsplit(['/', '\\']).next().unwrap_or("").to_ascii_lowercase();
        if entry.is_file() && base == EXE_NAME {
            found = Some(i);
            break;
        }
    }
    let i = found.ok_or_else(|| UpdateError::Corrupt(format!("the zip has no {EXE_NAME}")))?;
    let entry = z.by_index(i).map_err(|e| UpdateError::Corrupt(e.to_string()))?;
    if entry.size() > MAX_EXE {
        return Err(UpdateError::Corrupt("the program in the zip is too large".into()));
    }
    let mut data = Vec::new();
    entry.take(MAX_EXE + 1).read_to_end(&mut data)?;
    if data.len() as u64 > MAX_EXE {
        return Err(UpdateError::Corrupt("the program in the zip is too large".into()));
    }
    let mut o = std::fs::File::create(out)?;
    o.write_all(&data)?;
    o.flush()?;
    Ok(())
}

/// Remove what an earlier swap left (`rusty-wave.exe.old` and an unused `.new`); harmless anywhere else.
pub fn clean_leftovers(kind: &InstallKind) {
    if let InstallKind::WindowsPortable(exe) = kind {
        let _ = std::fs::remove_file(sibling(exe, ".old"));
        let _ = std::fs::remove_file(sibling(exe, ".new"));
    }
}

/// Carry out `finish`: start the program (or the installer) that continues. The caller then quits.
pub fn run_finish(finish: &Finish, args: &[OsString]) -> Result<(), UpdateError> {
    match finish {
        Finish::Restart { exe } => {
            let mut c = Command::new(exe);
            c.args(args);
            // The new AppImage sets these itself; the old mount's values must not leak into it.
            for v in ["APPDIR", "ARGV0", "OWD", "APPIMAGE"] {
                c.env_remove(v);
            }
            c.spawn().map_err(|e| UpdateError::Io(format!("cannot start {}: {e}", exe.display())))?;
            Ok(())
        }
        Finish::RunInstaller { path } => {
            Command::new(path)
                .args(["/SP-", "/SILENT", "/CLOSEAPPLICATIONS", "/RESTARTAPPLICATIONS"])
                .spawn()
                .map_err(|e| UpdateError::Io(format!("cannot start the installer: {e}")))?;
            Ok(())
        }
        Finish::SwapExe { new, current } => {
            let old = sibling(current, ".old");
            let _ = std::fs::remove_file(&old);
            // Windows cannot overwrite a running program but can rename it.
            std::fs::rename(current, &old)?;
            if let Err(e) = std::fs::rename(new, current) {
                let _ = std::fs::rename(&old, current);
                return Err(e.into());
            }
            if let Err(e) = Command::new(current).args(args).spawn() {
                let _ = std::fs::remove_file(current);
                let _ = std::fs::rename(&old, current);
                return Err(UpdateError::Io(format!("cannot start the new version: {e}")));
            }
            Ok(())
        }
    }
}

/// `Ok` if `name` is a plain file name (used when the manifest is read, before anything is offered).
pub fn fetch_name_ok(name: &str) -> Result<(), UpdateError> {
    safe_name(name).map(|_| ())
}
