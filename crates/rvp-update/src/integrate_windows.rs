//! The portable Windows build's integration with the shell: a per-user Start menu shortcut and per-user ("HKCU") file associations,
//! the same entries the installer writes (see `packaging/windows/rusty-wave.iss`) so "Open with" and Default apps list Rusty Wave.
//! Nothing here asks for administrator rights, nothing forces the default program, and removing undoes all of it.
//!
//! The registry sits behind [`Registry`], so the logic is tested on any system with an in-memory one.
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

/// The application id, used for the ProgId `<id>.Media`.
pub const APP_ID: &str = "io.github.idometeor.RustyWave";
/// The file types Rusty Wave registers for (kept equal to the installer's list by a test).
pub const EXTENSIONS: &[&str] = &[
    ".mp4", ".m4v", ".mkv", ".webm", ".mka", ".mp3", ".flac", ".ogg", ".oga", ".opus", ".wav", ".m4a",
    ".m4b", ".aac", ".m3u", ".m3u8", ".pls",
];

/// A tiny view of the current user's registry hive (`HKEY_CURRENT_USER`).
pub trait Registry {
    /// Set a string value (`name` empty is the key's default value), creating the key.
    fn set_string(&mut self, key: &str, name: &str, value: &str) -> io::Result<()>;
    /// Read a string value.
    fn get_string(&self, key: &str, name: &str) -> Option<String>;
    /// Delete a key and everything below it (missing is fine).
    fn delete_tree(&mut self, key: &str) -> io::Result<()>;
    /// Delete one value (missing is fine).
    fn delete_value(&mut self, key: &str, name: &str) -> io::Result<()>;
}

/// Whether the associations exist and name this executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Not registered.
    Absent,
    /// Registered for this executable.
    Current,
    /// Registered for another path (the program moved).
    Stale(String),
}

fn command(exe: &Path) -> String {
    format!("\"{}\" \"%1\"", exe.display())
}

fn prog_id() -> String {
    format!("{APP_ID}.Media")
}

const APP_KEY: &str = "Software\\Classes\\Applications\\rusty-wave.exe";
const CAPS_KEY: &str = "Software\\RustyWave\\Capabilities";

/// Register the associations for `exe`.
pub fn add_associations(reg: &mut dyn Registry, exe: &Path) -> io::Result<()> {
    let cmd = command(exe);
    let icon = format!("{},0", exe.display());
    let pid = prog_id();
    reg.set_string(APP_KEY, "FriendlyAppName", "Rusty Wave")?;
    reg.set_string(&format!("{APP_KEY}\\shell\\open\\command"), "", &cmd)?;
    reg.set_string(&format!("{APP_KEY}\\DefaultIcon"), "", &icon)?;
    let pk = format!("Software\\Classes\\{pid}");
    reg.set_string(&pk, "", "Media file (Rusty Wave)")?;
    reg.set_string(&format!("{pk}\\DefaultIcon"), "", &icon)?;
    reg.set_string(&format!("{pk}\\shell\\open\\command"), "", &cmd)?;
    reg.set_string(&format!("{pk}\\shell\\enqueue"), "MUIVerb", "Add to Rusty Wave queue")?;
    reg.set_string(&format!("{pk}\\shell\\enqueue\\command"), "", &cmd)?;
    reg.set_string(CAPS_KEY, "ApplicationName", "Rusty Wave")?;
    reg.set_string(
        CAPS_KEY,
        "ApplicationDescription",
        "Plays video and music, with a library, a queue and a visualizer.",
    )?;
    reg.set_string("Software\\RegisteredApplications", "RustyWave", CAPS_KEY)?;
    for ext in EXTENSIONS {
        reg.set_string(&format!("{CAPS_KEY}\\FileAssociations"), ext, &pid)?;
        reg.set_string(&format!("Software\\Classes\\{ext}\\OpenWithProgids"), &pid, "")?;
        reg.set_string(&format!("{APP_KEY}\\SupportedTypes"), ext, "")?;
    }
    Ok(())
}

/// Remove everything [`add_associations`] wrote.
pub fn remove_associations(reg: &mut dyn Registry) -> io::Result<()> {
    let pid = prog_id();
    for ext in EXTENSIONS {
        reg.delete_value(&format!("Software\\Classes\\{ext}\\OpenWithProgids"), &pid)?;
    }
    reg.delete_value("Software\\RegisteredApplications", "RustyWave")?;
    reg.delete_tree("Software\\RustyWave")?;
    reg.delete_tree(&format!("Software\\Classes\\{pid}"))?;
    reg.delete_tree(APP_KEY)?;
    Ok(())
}

/// What is registered, compared with `exe`.
pub fn associations_status(reg: &dyn Registry, exe: &Path) -> Status {
    match reg.get_string(&format!("{APP_KEY}\\shell\\open\\command"), "") {
        None => Status::Absent,
        Some(c) if c == command(exe) => Status::Current,
        Some(c) => Status::Stale(c),
    }
}

/// `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Rusty Wave.lnk`
pub fn shortcut_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA").filter(|v| !v.is_empty())?;
    Some(PathBuf::from(appdata).join("Microsoft/Windows/Start Menu/Programs/Rusty Wave.lnk"))
}

/// Make the Start menu shortcut to `exe` (through the system's own scripting host, which writes a proper shell link).
#[cfg(windows)]
pub fn create_shortcut(lnk: &Path, exe: &Path) -> io::Result<()> {
    use std::process::Command;
    if let Some(d) = lnk.parent() {
        std::fs::create_dir_all(d)?;
    }
    // The paths travel in environment variables, so nothing in them can break out of the script.
    let script = "$s=(New-Object -ComObject WScript.Shell).CreateShortcut($env:RW_LNK);\
                  $s.TargetPath=$env:RW_EXE;$s.WorkingDirectory=Split-Path $env:RW_EXE;$s.IconLocation=$env:RW_EXE+',0';\
                  $s.Description='Rusty Wave';$s.Save()";
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-Command",
            script,
        ])
        .env("RW_LNK", lnk)
        .env("RW_EXE", exe)
        .output()?;
    if out.status.success() && lnk.exists() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "could not create the shortcut: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

/// Only Windows has a Start menu.
#[cfg(not(windows))]
pub fn create_shortcut(_lnk: &Path, _exe: &Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "there is no Start menu on this system"))
}

/// Delete the Start menu shortcut (missing is fine).
pub fn remove_shortcut(lnk: &Path) -> io::Result<()> {
    match std::fs::remove_file(lnk) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// An in-memory registry (tests, and anything that wants a dry run).
#[derive(Debug, Default, Clone)]
pub struct MemRegistry {
    /// `key (lower-case) -> value name -> value`.
    pub keys: BTreeMap<String, BTreeMap<String, String>>,
}

impl MemRegistry {
    fn norm(key: &str) -> String {
        key.to_ascii_lowercase()
    }
}

impl Registry for MemRegistry {
    fn set_string(&mut self, key: &str, name: &str, value: &str) -> io::Result<()> {
        self.keys.entry(Self::norm(key)).or_default().insert(name.to_string(), value.to_string());
        Ok(())
    }
    fn get_string(&self, key: &str, name: &str) -> Option<String> {
        self.keys.get(&Self::norm(key))?.get(name).cloned()
    }
    fn delete_tree(&mut self, key: &str) -> io::Result<()> {
        let k = Self::norm(key);
        let below = format!("{k}\\");
        self.keys.retain(|name, _| *name != k && !name.starts_with(&below));
        Ok(())
    }
    fn delete_value(&mut self, key: &str, name: &str) -> io::Result<()> {
        if let Some(v) = self.keys.get_mut(&Self::norm(key)) {
            v.remove(name);
            if v.is_empty() {
                self.keys.remove(&Self::norm(key));
            }
        }
        Ok(())
    }
}

/// The real `HKEY_CURRENT_USER`.
#[cfg(windows)]
pub struct HkcuRegistry;

#[cfg(windows)]
mod hkcu {
    use super::{HkcuRegistry, Registry};
    use std::io;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW, RegSetValueExW,
    };
    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn check(code: u32) -> io::Result<()> {
        if code == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(code as i32)) }
    }

    /// Tell Explorer that file associations changed (so Default apps and "Open with" refresh).
    pub fn notify_assoc_changed() {
        // SAFETY: no pointers are passed.
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, null(), null()) };
    }

    impl Registry for HkcuRegistry {
        fn set_string(&mut self, key: &str, name: &str, value: &str) -> io::Result<()> {
            let k = wide(key);
            let n = wide(name);
            let v = wide(value);
            let mut h: HKEY = null_mut();
            // SAFETY: all strings are NUL-terminated and outlive the calls; `h` is closed below.
            unsafe {
                check(RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    k.as_ptr(),
                    0,
                    null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_SET_VALUE,
                    null(),
                    &mut h,
                    null_mut(),
                ))?;
                let r = RegSetValueExW(
                    h,
                    if name.is_empty() { null() } else { n.as_ptr() },
                    0,
                    REG_SZ,
                    v.as_ptr().cast(),
                    (v.len() * 2) as u32,
                );
                RegCloseKey(h);
                check(r)
            }
        }

        fn get_string(&self, key: &str, name: &str) -> Option<String> {
            let k = wide(key);
            let n = wide(name);
            let mut size = 0u32;
            // SAFETY: strings are NUL-terminated; the buffer is sized by the first call.
            unsafe {
                let np = if name.is_empty() { null() } else { n.as_ptr() };
                if RegGetValueW(
                    HKEY_CURRENT_USER,
                    k.as_ptr(),
                    np,
                    RRF_RT_REG_SZ,
                    null_mut(),
                    null_mut(),
                    &mut size,
                ) != ERROR_SUCCESS
                {
                    return None;
                }
                let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
                let mut size2 = (buf.len() * 2) as u32;
                if RegGetValueW(
                    HKEY_CURRENT_USER,
                    k.as_ptr(),
                    np,
                    RRF_RT_REG_SZ,
                    null_mut(),
                    buf.as_mut_ptr().cast(),
                    &mut size2,
                ) != ERROR_SUCCESS
                {
                    return None;
                }
                let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                Some(String::from_utf16_lossy(&buf[..len]))
            }
        }

        fn delete_tree(&mut self, key: &str) -> io::Result<()> {
            let k = wide(key);
            // SAFETY: NUL-terminated string.
            let r = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr()) };
            if r == ERROR_FILE_NOT_FOUND { Ok(()) } else { check(r) }
        }

        fn delete_value(&mut self, key: &str, name: &str) -> io::Result<()> {
            let k = wide(key);
            let n = wide(name);
            // SAFETY: NUL-terminated strings.
            let r = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr()) };
            if r == ERROR_FILE_NOT_FOUND { Ok(()) } else { check(r) }
        }
    }
}

/// Tell Explorer the associations changed (no-op off Windows).
pub fn notify_assoc_changed() {
    #[cfg(windows)]
    hkcu::notify_assoc_changed();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe() -> PathBuf {
        PathBuf::from("C:\\Users\\u\\Apps\\rusty-wave.exe")
    }

    #[test]
    fn the_extension_list_is_the_installers() {
        let iss = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packaging/windows/rusty-wave.iss"
        ))
        .unwrap();
        let mut theirs: Vec<String> = iss
            .lines()
            .filter(|l| l.contains("\\FileAssociations\""))
            .filter_map(|l| l.split("ValueName: \"").nth(1)?.split('"').next().map(str::to_string))
            .collect();
        theirs.sort();
        let mut ours: Vec<String> = EXTENSIONS.iter().map(|s| s.to_string()).collect();
        ours.sort();
        assert_eq!(ours, theirs);
    }

    #[test]
    fn add_registers_the_program_and_every_type_then_remove_leaves_nothing() {
        let mut r = MemRegistry::default();
        assert_eq!(associations_status(&r, &exe()), Status::Absent);
        add_associations(&mut r, &exe()).unwrap();
        assert_eq!(associations_status(&r, &exe()), Status::Current);
        assert_eq!(
            r.get_string("Software\\Classes\\io.github.idometeor.RustyWave.Media\\shell\\open\\command", "")
                .unwrap(),
            "\"C:\\Users\\u\\Apps\\rusty-wave.exe\" \"%1\""
        );
        assert_eq!(r.get_string("Software\\RegisteredApplications", "RustyWave").unwrap(), CAPS_KEY);
        for ext in EXTENSIONS {
            assert_eq!(
                r.get_string(&format!("{CAPS_KEY}\\FileAssociations"), ext).unwrap(),
                "io.github.idometeor.RustyWave.Media"
            );
            assert!(
                r.get_string(
                    &format!("Software\\Classes\\{ext}\\OpenWithProgids"),
                    "io.github.idometeor.RustyWave.Media"
                )
                .is_some()
            );
            assert!(r.get_string(&format!("{APP_KEY}\\SupportedTypes"), ext).is_some());
        }
        remove_associations(&mut r).unwrap();
        assert!(r.keys.is_empty(), "left behind: {:?}", r.keys.keys().collect::<Vec<_>>());
        // Removing again, and from nothing, is fine.
        remove_associations(&mut r).unwrap();
    }

    #[test]
    fn other_programs_entries_for_the_same_types_survive_a_removal() {
        let mut r = MemRegistry::default();
        r.set_string("Software\\Classes\\.mp4\\OpenWithProgids", "VLC.mp4", "").unwrap();
        r.set_string("Software\\Classes\\.mp4", "", "mp4file").unwrap();
        add_associations(&mut r, &exe()).unwrap();
        remove_associations(&mut r).unwrap();
        assert!(r.get_string("Software\\Classes\\.mp4\\OpenWithProgids", "VLC.mp4").is_some());
        assert_eq!(r.get_string("Software\\Classes\\.mp4", "").unwrap(), "mp4file");
        assert_eq!(r.keys.len(), 2);
    }

    #[test]
    fn a_moved_program_shows_as_stale_and_registering_again_repairs_it() {
        let mut r = MemRegistry::default();
        add_associations(&mut r, &exe()).unwrap();
        let moved = PathBuf::from("D:\\Portable\\rusty-wave.exe");
        assert!(matches!(associations_status(&r, &moved), Status::Stale(c) if c.contains("C:\\Users")));
        add_associations(&mut r, &moved).unwrap();
        assert_eq!(associations_status(&r, &moved), Status::Current);
    }

    #[test]
    fn the_shortcut_goes_to_the_per_user_start_menu_and_removal_tolerates_a_missing_one() {
        let d = std::env::temp_dir().join(format!("rvp-lnk-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert!(remove_shortcut(&d.join("nothing.lnk")).is_ok());
        std::fs::write(d.join("a.lnk"), b"x").unwrap();
        remove_shortcut(&d.join("a.lnk")).unwrap();
        assert!(!d.join("a.lnk").exists());
        // SAFETY: tests in this process that read APPDATA do not run concurrently with this one.
        let saved = std::env::var_os("APPDATA");
        unsafe { std::env::set_var("APPDATA", "C:\\Users\\u\\AppData\\Roaming") };
        let p = shortcut_path().unwrap();
        match saved {
            Some(v) => unsafe { std::env::set_var("APPDATA", v) },
            None => unsafe { std::env::remove_var("APPDATA") },
        }
        assert!(p.to_string_lossy().ends_with("Start Menu/Programs/Rusty Wave.lnk"), "{p:?}");
    }
}
