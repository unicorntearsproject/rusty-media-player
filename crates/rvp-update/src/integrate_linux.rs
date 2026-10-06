//! Adding an AppImage to the desktop's application menu: a `.desktop` file and icons under `~/.local/share`.
//!
//! Only files this module wrote are ever changed or removed: ours carry `X-RustyWave-Integrated=true` and the path of the AppImage they
//! launch. A desktop file of the same name that lacks the marker (a package's, or the user's own) is left alone.
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The application id: the file names, the icon name, the window class.
pub const APP_ID: &str = "io.github.unicorntearsproject.RustyWave";
/// The marker line that says this desktop file is ours.
const MARKER: &str = "X-RustyWave-Integrated=true";
const PATH_KEY: &str = "X-RustyWave-AppImage=";

/// The shared desktop file the packages ship, used as the template (same name, categories and MIME types).
const TEMPLATE: &str =
    include_str!("../../../packaging/shared/io.github.unicorntearsproject.RustyWave.desktop");

/// The icon sizes embedded; the desktop scales these for the others.
pub const ICONS: &[(u32, &[u8])] = &[
    (
        16,
        include_bytes!(
            "../../../packaging/icons/hicolor/16x16/apps/io.github.unicorntearsproject.RustyWave.png"
        ),
    ),
    (
        32,
        include_bytes!(
            "../../../packaging/icons/hicolor/32x32/apps/io.github.unicorntearsproject.RustyWave.png"
        ),
    ),
    (
        48,
        include_bytes!(
            "../../../packaging/icons/hicolor/48x48/apps/io.github.unicorntearsproject.RustyWave.png"
        ),
    ),
    (
        64,
        include_bytes!(
            "../../../packaging/icons/hicolor/64x64/apps/io.github.unicorntearsproject.RustyWave.png"
        ),
    ),
    (
        128,
        include_bytes!(
            "../../../packaging/icons/hicolor/128x128/apps/io.github.unicorntearsproject.RustyWave.png"
        ),
    ),
    (
        256,
        include_bytes!(
            "../../../packaging/icons/hicolor/256x256/apps/io.github.unicorntearsproject.RustyWave.png"
        ),
    ),
];

/// Whether the menu entry exists and points here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// There is no entry of ours.
    Absent,
    /// Our entry launches this AppImage.
    Current,
    /// Our entry launches another path (the AppImage moved).
    Stale(String),
    /// A desktop file of that name exists that is not ours; nothing is touched.
    Foreign,
}

/// Runs the optional cache-refreshing tools.
pub trait Runner {
    /// Run `program args`; failures (the tool is not installed) are ignored by the caller.
    fn run(&self, program: &str, args: &[&str]);
}

/// Really runs them.
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[&str]) {
        let _ = Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// `$XDG_DATA_HOME`, else `~/.local/share`.
pub fn data_home() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return Some(d);
    }
    std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|h| PathBuf::from(h).join(".local/share"))
}

fn desktop_path(data_home: &Path) -> PathBuf {
    data_home.join("applications").join(format!("{APP_ID}.desktop"))
}

fn icon_path(data_home: &Path, size: u32) -> PathBuf {
    data_home.join("icons/hicolor").join(format!("{size}x{size}/apps/{APP_ID}.png"))
}

/// The value of an Exec argument: quoted when it has reserved characters, then the desktop-file string escapes.
pub fn exec_quote(path: &str) -> String {
    const RESERVED: &str = " \t\n\"'\\><~|&;$*?#()`";
    let quoted = if path.chars().any(|c| RESERVED.contains(c)) {
        let mut q = String::from("\"");
        for c in path.chars() {
            if matches!(c, '"' | '`' | '$' | '\\') {
                q.push('\\');
            }
            q.push(c);
        }
        q.push('"');
        q
    } else {
        path.to_string()
    };
    // The key-value layer: a backslash is written twice.
    quoted.replace('\\', "\\\\")
}

/// The desktop file for an AppImage at `appimage`.
pub fn desktop_file(appimage: &Path) -> String {
    // A control character in a path cannot be written into a line-based file; `add` refuses such paths, this keeps the file sane anyway.
    let p = appimage.to_string_lossy().replace(|c: char| c.is_control(), " ");
    let mut out = String::new();
    for line in TEMPLATE.lines() {
        if line.starts_with("Exec=") {
            out.push_str(&format!("Exec={} %U\n", exec_quote(&p)));
        } else if line.starts_with("TryExec=") {
            out.push_str(&format!("TryExec={}\n", p.replace('\\', "\\\\")));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str(MARKER);
    out.push('\n');
    out.push_str(&format!("{PATH_KEY}{}\n", p.replace('\\', "\\\\").replace('\n', " ")));
    out
}

/// The state of the menu entry for the AppImage at `appimage`.
pub fn status(data_home: &Path, appimage: &Path) -> Status {
    let Ok(text) = fs::read_to_string(desktop_path(data_home)) else { return Status::Absent };
    if !text.lines().any(|l| l.trim() == MARKER) {
        return Status::Foreign;
    }
    let want = appimage.to_string_lossy().replace('\\', "\\\\").replace('\n', " ");
    match text.lines().find_map(|l| l.strip_prefix(PATH_KEY)) {
        Some(p) if p == want => Status::Current,
        Some(p) => Status::Stale(p.replace("\\\\", "\\")),
        None => Status::Stale(String::new()),
    }
}

fn refresh(data_home: &Path, runner: &dyn Runner) {
    runner.run("update-desktop-database", &["-q", &data_home.join("applications").to_string_lossy()]);
    runner.run(
        "gtk-update-icon-cache",
        &["-q", "-f", "-t", &data_home.join("icons/hicolor").to_string_lossy()],
    );
}

/// Write the desktop file and the icons for `appimage` (replacing our own entry, never a foreign one) and refresh the caches.
pub fn add(data_home: &Path, appimage: &Path, runner: &dyn Runner) -> io::Result<()> {
    if appimage.to_string_lossy().chars().any(char::is_control) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the AppImage's path has a control character in it",
        ));
    }
    if status(data_home, appimage) == Status::Foreign {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "another program already has a menu entry with this name; it was left alone",
        ));
    }
    let d = desktop_path(data_home);
    fs::create_dir_all(d.parent().unwrap_or(data_home))?;
    write_atomic(&d, desktop_file(appimage).as_bytes())?;
    for (size, png) in ICONS {
        let p = icon_path(data_home, *size);
        fs::create_dir_all(p.parent().unwrap_or(data_home))?;
        write_atomic(&p, png)?;
    }
    refresh(data_home, runner);
    Ok(())
}

/// Remove our desktop file and icons (a foreign file is left alone) and refresh the caches.
pub fn remove(data_home: &Path, runner: &dyn Runner) -> io::Result<()> {
    let d = desktop_path(data_home);
    if let Ok(text) = fs::read_to_string(&d) {
        if !text.lines().any(|l| l.trim() == MARKER) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "that menu entry was not made by Rusty Wave; it was left alone",
            ));
        }
        fs::remove_file(&d)?;
    }
    for (size, _) in ICONS {
        match fs::remove_file(icon_path(data_home, *size)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    refresh(data_home, runner);
    Ok(())
}

/// Point our entry at `appimage` when it names another path (the AppImage moved). Returns whether it changed anything.
pub fn repair(data_home: &Path, appimage: &Path, runner: &dyn Runner) -> io::Result<bool> {
    match status(data_home, appimage) {
        Status::Stale(_) => {
            add(data_home, appimage, runner)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Recorder(RefCell<Vec<String>>);
    impl Runner for Recorder {
        fn run(&self, program: &str, args: &[&str]) {
            self.0.borrow_mut().push(format!("{program} {}", args.join(" ")));
        }
    }

    fn home(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rvp-integ-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn exec_quoting_follows_the_desktop_entry_spec() {
        assert_eq!(exec_quote("/home/u/Rusty-Wave.AppImage"), "/home/u/Rusty-Wave.AppImage");
        assert_eq!(
            exec_quote("/home/u/My Apps/Rusty Wave.AppImage"),
            "\"/home/u/My Apps/Rusty Wave.AppImage\""
        );
        // A double quote and a dollar sign inside a quoted path are escaped, and the key-value layer doubles the backslashes.
        assert_eq!(exec_quote("/a b/\"x\"$y"), "\"/a b/\\\\\"x\\\\\"\\\\$y\"");
    }

    #[test]
    fn the_desktop_file_launches_the_appimage_and_matches_the_window_class() {
        let t = desktop_file(Path::new("/home/u/My Apps/Rusty Wave.AppImage"));
        assert!(t.contains("\nExec=\"/home/u/My Apps/Rusty Wave.AppImage\" %U\n"), "{t}");
        assert!(t.contains("\nTryExec=/home/u/My Apps/Rusty Wave.AppImage\n"));
        assert!(t.contains(&format!("\nStartupWMClass={APP_ID}\n")));
        assert!(t.contains(&format!("\nIcon={APP_ID}\n")));
        assert!(t.contains("MimeType=video/mp4;") && t.contains("audio/flac;"));
        assert!(t.contains(MARKER));
        // Exactly one Exec line.
        assert_eq!(t.lines().filter(|l| l.starts_with("Exec=")).count(), 1);
    }

    #[test]
    fn add_writes_the_files_and_refreshes_the_caches_and_remove_takes_them_away() {
        let h = home("add");
        let app = Path::new("/opt/Rusty Wave.AppImage");
        let r = Recorder::default();
        assert_eq!(status(&h, app), Status::Absent);
        add(&h, app, &r).unwrap();
        assert_eq!(status(&h, app), Status::Current);
        for (size, png) in ICONS {
            assert_eq!(fs::read(icon_path(&h, *size)).unwrap(), *png);
        }
        assert_eq!(r.0.borrow().len(), 2);
        assert!(r.0.borrow()[0].starts_with("update-desktop-database"));
        assert!(r.0.borrow()[1].starts_with("gtk-update-icon-cache"));
        // Doing it twice is harmless.
        add(&h, app, &r).unwrap();
        assert_eq!(status(&h, app), Status::Current);
        remove(&h, &r).unwrap();
        assert_eq!(status(&h, app), Status::Absent);
        assert!(!icon_path(&h, 256).exists());
        // Removing what is not there is fine.
        remove(&h, &r).unwrap();
    }

    #[test]
    fn a_moved_appimage_is_repaired_and_the_same_one_is_not_touched() {
        let h = home("repair");
        let r = Recorder::default();
        let old = Path::new("/old/place/Rusty Wave.AppImage");
        let new = Path::new("/new/place/Rusty Wave.AppImage");
        add(&h, old, &r).unwrap();
        assert!(!repair(&h, old, &r).unwrap());
        assert_eq!(status(&h, new), Status::Stale(old.to_string_lossy().into_owned()));
        assert!(repair(&h, new, &r).unwrap());
        assert_eq!(status(&h, new), Status::Current);
        assert!(fs::read_to_string(desktop_path(&h)).unwrap().contains("/new/place/"));
        // Nothing to repair when there is no entry.
        let h2 = home("repair2");
        assert!(!repair(&h2, new, &r).unwrap());
        assert!(!desktop_path(&h2).exists());
    }

    #[test]
    fn a_desktop_file_that_is_not_ours_is_never_touched() {
        let h = home("foreign");
        let r = Recorder::default();
        let theirs = "[Desktop Entry]\nType=Application\nName=Rusty Wave\nExec=rusty-wave %U\n";
        fs::create_dir_all(h.join("applications")).unwrap();
        fs::write(desktop_path(&h), theirs).unwrap();
        let app = Path::new("/x/Rusty Wave.AppImage");
        assert_eq!(status(&h, app), Status::Foreign);
        assert!(add(&h, app, &r).is_err());
        assert!(remove(&h, &r).is_err());
        assert!(!repair(&h, app, &r).unwrap());
        assert_eq!(fs::read_to_string(desktop_path(&h)).unwrap(), theirs);
        assert!(!icon_path(&h, 256).exists());
        assert!(r.0.borrow().is_empty());
    }

    #[test]
    fn a_path_with_a_newline_cannot_inject_keys() {
        let evil = Path::new("/x/evil\nExec=rm -rf\n.AppImage");
        let t = desktop_file(evil);
        assert_eq!(t.lines().filter(|l| l.starts_with("Exec=")).count(), 1, "{t}");
        let h = home("newline");
        assert!(add(&h, evil, &Recorder::default()).is_err());
        assert!(!desktop_path(&h).exists());
    }
}
