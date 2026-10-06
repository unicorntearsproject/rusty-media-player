//! Making the app the default media player: what each system lets a program do.
//!
//! * **Linux** (the XDG way): per-user `mimeapps.list` in `$XDG_CONFIG_HOME` (`~/.config`), `[Default Applications]` lines of
//!   `media/type=app.desktop;`. This is what `xdg-mime default` writes; it is done here directly so it works without the tool and is
//!   tested without a desktop. The desktop entry must exist (a package, Flatpak or the AppImage's menu entry).
//! * **Windows** does not let a program take the defaults over silently (since Windows 8 only the user may, in Settings). The program
//!   registers itself (the associations the installer or the portable integration write) and opens the *Default apps* page; the user
//!   confirms there. The UI says so.
//! * **macOS**: `LSSetDefaultRoleHandlerForContentType` for each type's UTI (the system asks the user to confirm once).
//!
//! The pure parts are tested everywhere; the calls into the system are small and live behind `cfg`.
use std::io;
use std::path::{Path, PathBuf};

/// The bundle id of the macOS app.
pub const MACOS_BUNDLE_ID: &str = "io.github.idometeor.RustyWave";

/// The desktop entry id of the app (`io.github.idometeor.RustyWave.desktop`).
pub const DESKTOP_ID: &str = "io.github.idometeor.RustyWave.desktop";

/// `mimeapps.list` with `desktop_id` made the default for each of `mimes`: its `[Default Applications]` line becomes
/// `mime=desktop_id;` followed by whatever else was there (without a repeat). Everything else in the file is kept as it was, including
/// comments; a missing section or file is made.
pub fn with_defaults(text: &str, desktop_id: &str, mimes: &[&str]) -> String {
    const SECTION: &str = "[Default Applications]";
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let start = match lines.iter().position(|l| l.trim() == SECTION) {
        Some(i) => i,
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(SECTION.to_string());
            lines.len() - 1
        }
    };
    for mime in mimes {
        // Where the section ends (the next one starts, or the file does); looked up again as lines are inserted.
        let end = lines[start + 1..]
            .iter()
            .position(|l| l.trim_start().starts_with('['))
            .map_or(lines.len(), |p| start + 1 + p);
        let prefix = format!("{mime}=");
        match (start + 1..end).find(|&i| lines[i].trim_start().starts_with(&prefix)) {
            Some(i) => {
                let mut line = format!("{prefix}{desktop_id};");
                for other in lines[i].trim_start()[prefix.len()..]
                    .split(';')
                    .map(str::trim)
                    .filter(|a| !a.is_empty() && *a != desktop_id)
                {
                    line.push_str(other);
                    line.push(';');
                }
                lines[i] = line;
            }
            None => {
                // At the end of the section, above any blank lines that separate it from the next.
                let mut at = end;
                while at > start + 1 && lines[at - 1].trim().is_empty() {
                    at -= 1;
                }
                lines.insert(at, format!("{prefix}{desktop_id};"));
            }
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Whether `mimeapps.list` text names `desktop_id` first for `mime`.
pub fn is_default(text: &str, desktop_id: &str, mime: &str) -> bool {
    let mut in_section = false;
    for l in text.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            in_section = l == "[Default Applications]";
        } else if in_section
            && let Some(v) = l.strip_prefix(&format!("{mime}="))
        {
            return v.split(';').map(str::trim).find(|a| !a.is_empty()) == Some(desktop_id);
        }
    }
    false
}

/// `$XDG_CONFIG_HOME` (or `~/.config`).
pub fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|h| PathBuf::from(h).join(".config")))
}

/// Make the app the default for `mimes` in the user's `mimeapps.list` under `config_home`. The file is written in one step (a temporary
/// file, then a rename) so a crash never leaves half of it. Returns the number of media types set.
pub fn set_linux_defaults(config_home: &Path, desktop_id: &str, mimes: &[&str]) -> io::Result<usize> {
    let path = config_home.join("mimeapps.list");
    let old = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let new = with_defaults(&old, desktop_id, mimes);
    std::fs::create_dir_all(config_home)?;
    let tmp = config_home.join(".mimeapps.list.rusty-wave.tmp");
    std::fs::write(&tmp, new)?;
    std::fs::rename(&tmp, &path)?;
    Ok(mimes.len())
}

/// Open Windows' *Default apps* page for Rusty Wave (the user confirms the choices there).
#[cfg(windows)]
pub fn open_windows_default_apps() -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("cmd")
        .args(["/C", "start", "", "ms-settings:defaultapps?registeredAppUser=Rusty%20Wave"])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .status()
        .and_then(|s| if s.success() { Ok(()) } else { Err(io::Error::other("Windows would not open its settings")) })
}

/// Not Windows.
#[cfg(not(windows))]
pub fn open_windows_default_apps() -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "this is not Windows"))
}

/// Make the app (by bundle id) the handler of every role for the types with these file name extensions. Returns how many were set.
#[cfg(target_os = "macos")]
pub fn set_macos_defaults(bundle_id: &str, extensions: &[&str]) -> io::Result<usize> {
    use std::ffi::{CString, c_char, c_void};
    type CFStringRef = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    const ROLES_ALL: u32 = 0xFFFF_FFFF;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFAllocatorDefault: *const c_void;
        fn CFStringCreateWithCString(alloc: *const c_void, c: *const c_char, enc: u32) -> CFStringRef;
        fn CFRelease(p: *const c_void);
    }
    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        static kUTTagClassFilenameExtension: CFStringRef;
        fn UTTypeCreatePreferredIdentifierForTag(class: CFStringRef, tag: CFStringRef, conforming: CFStringRef) -> CFStringRef;
        fn LSSetDefaultRoleHandlerForContentType(content_type: CFStringRef, roles: u32, handler: CFStringRef) -> i32;
    }
    let cf = |s: &str| -> io::Result<CFStringRef> {
        let c = CString::new(s).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a NUL in a name"))?;
        // SAFETY: `c` is a valid NUL-terminated string for the call; the result is released by the caller.
        let r = unsafe { CFStringCreateWithCString(kCFAllocatorDefault, c.as_ptr(), UTF8) };
        if r.is_null() { Err(io::Error::other("out of memory")) } else { Ok(r) }
    };
    let handler = cf(bundle_id)?;
    let mut done = 0;
    for ext in extensions {
        let tag = cf(ext)?;
        // SAFETY: both strings are valid CFStrings; a null result (an unknown extension) is handled.
        let uti = unsafe { UTTypeCreatePreferredIdentifierForTag(kUTTagClassFilenameExtension, tag, std::ptr::null()) };
        // SAFETY: `tag` was created above and is not used again.
        unsafe { CFRelease(tag) };
        if uti.is_null() {
            continue;
        }
        // SAFETY: `uti` and `handler` are valid CFStrings.
        let status = unsafe { LSSetDefaultRoleHandlerForContentType(uti, ROLES_ALL, handler) };
        // SAFETY: `uti` is ours (a "Create" result) and is not used again.
        unsafe { CFRelease(uti) };
        if status == 0 {
            done += 1;
        }
    }
    // SAFETY: `handler` was created above and is not used again.
    unsafe { CFRelease(handler) };
    if done == 0 { Err(io::Error::other("macOS did not accept any of the types")) } else { Ok(done) }
}

/// Not macOS.
#[cfg(not(target_os = "macos"))]
pub fn set_macos_defaults(_bundle_id: &str, _extensions: &[&str]) -> io::Result<usize> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "this is not macOS"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = DESKTOP_ID;

    #[test]
    fn a_new_file_gets_the_section_and_the_lines() {
        let t = with_defaults("", ID, &["video/mp4", "audio/flac"]);
        assert_eq!(t, format!("[Default Applications]\nvideo/mp4={ID};\naudio/flac={ID};\n"));
        assert!(is_default(&t, ID, "video/mp4") && !is_default(&t, ID, "video/webm"));
    }

    #[test]
    fn existing_choices_are_kept_behind_ours_and_nothing_else_changes() {
        let old = "# my choices\n[Added Associations]\nvideo/mp4=vlc.desktop;\n\n[Default Applications]\nvideo/mp4=mpv.desktop;vlc.desktop;\nimage/png=eog.desktop;\n\n[Removed Associations]\naudio/mpeg=x.desktop;\n";
        let t = with_defaults(old, ID, &["video/mp4", "audio/mpeg"]);
        assert!(t.starts_with("# my choices\n[Added Associations]\nvideo/mp4=vlc.desktop;\n"), "{t}");
        assert!(t.contains(&format!("video/mp4={ID};mpv.desktop;vlc.desktop;\n")), "{t}");
        assert!(t.contains("image/png=eog.desktop;\n"), "{t}");
        // A new type lands inside the section, not in the next one.
        let sect = t.split("[Default Applications]").nth(1).unwrap().split("[Removed Associations]").next().unwrap();
        assert!(sect.contains(&format!("audio/mpeg={ID};")), "{t}");
        assert!(t.contains("[Removed Associations]\naudio/mpeg=x.desktop;\n"), "{t}");
    }

    #[test]
    fn setting_twice_changes_nothing_and_ours_is_never_listed_twice() {
        let once = with_defaults("[Default Applications]\nvideo/mp4=a.desktop;\n", ID, &["video/mp4"]);
        let twice = with_defaults(&once, ID, &["video/mp4"]);
        assert_eq!(once, twice);
        assert_eq!(twice.matches(ID).count(), 1);
        // Already behind others: it moves to the front.
        let behind = with_defaults(&format!("[Default Applications]\nvideo/mp4=a.desktop;{ID};\n"), ID, &["video/mp4"]);
        assert!(behind.contains(&format!("video/mp4={ID};a.desktop;\n")), "{behind}");
    }

    #[test]
    fn the_file_is_written_in_the_config_folder() {
        let dir = std::env::temp_dir().join(format!("rvp-mimeapps-{}", std::process::id()));
        let n = set_linux_defaults(&dir, ID, &["video/mp4", "audio/flac"]).unwrap();
        assert_eq!(n, 2);
        let text = std::fs::read_to_string(dir.join("mimeapps.list")).unwrap();
        assert!(is_default(&text, ID, "audio/flac"), "{text}");
        assert!(!dir.join(".mimeapps.list.rusty-wave.tmp").exists());
        let _ = std::fs::remove_file(dir.join("mimeapps.list"));
        let _ = std::fs::remove_dir(&dir);
    }
}
