//! How this copy of the program was installed, which decides how it is updated.
use std::path::{Path, PathBuf};

/// The operating system family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// Linux.
    Linux,
    /// Windows.
    Windows,
    /// macOS.
    MacOs,
    /// Anything else.
    Other,
}

impl Os {
    /// The system this was compiled for.
    pub fn current() -> Os {
        if cfg!(target_os = "linux") {
            Os::Linux
        } else if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Other
        }
    }
}

/// What detection looks at (injectable, so every case is testable on one machine).
#[derive(Debug, Clone)]
pub struct Env {
    /// The system.
    pub os: Os,
    /// `$APPIMAGE`: the path of the AppImage file we run from.
    pub appimage: Option<PathBuf>,
    /// Running inside Flatpak's sandbox.
    pub flatpak: bool,
    /// The running executable.
    pub exe: PathBuf,
}

impl Env {
    /// The real environment.
    pub fn current() -> Env {
        Env {
            os: Os::current(),
            appimage: std::env::var_os("APPIMAGE").filter(|v| !v.is_empty()).map(PathBuf::from),
            flatpak: std::env::var_os("FLATPAK_ID").is_some_and(|v| !v.is_empty())
                || Path::new("/.flatpak-info").exists(),
            exe: std::env::current_exe().unwrap_or_default(),
        }
    }
}

/// How the program is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// An AppImage file (the path replaced by an update).
    AppImage(PathBuf),
    /// Installed from Flatpak.
    Flatpak,
    /// Installed by the system's package manager (.deb or .rpm).
    Package,
    /// The Windows Setup program installed it.
    WindowsInstaller,
    /// The portable Windows executable (the path replaced by an update).
    WindowsPortable(PathBuf),
    /// An app bundle on macOS.
    MacApp,
    /// A tarball, a development build or anything else.
    Other,
}

impl InstallKind {
    /// Look at `env`; `exists` answers whether a path exists (so tests need no files).
    pub fn detect(env: &Env, exists: &dyn Fn(&Path) -> bool) -> InstallKind {
        match env.os {
            Os::Linux => {
                if env.flatpak {
                    InstallKind::Flatpak
                } else if let Some(p) = env.appimage.as_ref().filter(|p| exists(p)) {
                    InstallKind::AppImage(p.clone())
                } else if env.exe.starts_with("/usr/bin") {
                    InstallKind::Package
                } else {
                    InstallKind::Other
                }
            }
            Os::Windows => {
                let beside = env.exe.parent().map(|d| d.join("unins000.exe"));
                if beside.as_deref().is_some_and(exists) {
                    InstallKind::WindowsInstaller
                } else {
                    InstallKind::WindowsPortable(env.exe.clone())
                }
            }
            Os::MacOs => InstallKind::MacApp,
            Os::Other => InstallKind::Other,
        }
    }

    /// The manifest key of the file that updates this kind of install, if there is one.
    pub fn manifest_key(&self) -> Option<&'static str> {
        match self {
            InstallKind::AppImage(_) => Some("linux-appimage"),
            InstallKind::WindowsInstaller => Some("windows-installer"),
            InstallKind::WindowsPortable(_) => Some("windows-portable"),
            InstallKind::MacApp => Some("macos-dmg"),
            InstallKind::Package | InstallKind::Flatpak | InstallKind::Other => None,
        }
    }

    /// True when the program can download and install the update itself.
    pub fn can_self_update(&self) -> bool {
        matches!(
            self,
            InstallKind::AppImage(_) | InstallKind::WindowsInstaller | InstallKind::WindowsPortable(_)
        )
    }

    /// What to tell the user when the program cannot update itself.
    pub fn manual_message(&self) -> &'static str {
        match self {
            InstallKind::Package => "Update through your package manager (apt, dnf or your software centre).",
            InstallKind::Flatpak => "Update through Flatpak or your software store.",
            InstallKind::MacApp => "Download the new version and drag it over the old one in Applications.",
            _ => "Download the new version from the Rusty Wave site.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(os: Os, exe: &str) -> Env {
        Env { os, appimage: None, flatpak: false, exe: exe.into() }
    }

    #[test]
    fn linux_kinds() {
        let yes = |_: &Path| true;
        let no = |_: &Path| false;
        let mut e = env(Os::Linux, "/tmp/.mount_x/usr/bin/rusty-wave");
        e.appimage = Some("/home/u/Apps/Rusty Wave.AppImage".into());
        assert_eq!(
            InstallKind::detect(&e, &yes),
            InstallKind::AppImage("/home/u/Apps/Rusty Wave.AppImage".into())
        );
        // $APPIMAGE naming a file that is gone is not an AppImage run we can replace.
        assert_eq!(InstallKind::detect(&e, &no), InstallKind::Other);
        e.flatpak = true;
        assert_eq!(InstallKind::detect(&e, &yes), InstallKind::Flatpak);
        assert_eq!(InstallKind::detect(&env(Os::Linux, "/usr/bin/rusty-wave"), &no), InstallKind::Package);
        assert_eq!(
            InstallKind::detect(&env(Os::Linux, "/home/u/src/target/debug/rusty-wave"), &no),
            InstallKind::Other
        );
    }

    #[test]
    fn windows_kinds() {
        let installed = |p: &Path| p.ends_with("unins000.exe");
        let none = |_: &Path| false;
        let e = env(Os::Windows, "C:\\Users\\u\\AppData\\Local\\Programs\\Rusty Wave\\rusty-wave.exe");
        // The test runs on any OS, where a Windows path is one component; the parent logic is what matters.
        let e2 = Env { exe: PathBuf::from("/prog/rusty-wave.exe"), ..e.clone() };
        assert_eq!(InstallKind::detect(&e2, &installed), InstallKind::WindowsInstaller);
        assert_eq!(
            InstallKind::detect(&e2, &none),
            InstallKind::WindowsPortable("/prog/rusty-wave.exe".into())
        );
        assert_eq!(
            InstallKind::detect(
                &env(Os::MacOs, "/Applications/Rusty Wave.app/Contents/MacOS/rusty-wave"),
                &none
            ),
            InstallKind::MacApp
        );
    }

    #[test]
    fn who_updates_itself() {
        assert!(InstallKind::AppImage("/x".into()).can_self_update());
        assert!(InstallKind::WindowsInstaller.can_self_update());
        assert!(!InstallKind::Package.can_self_update());
        assert!(!InstallKind::Flatpak.can_self_update());
        assert!(InstallKind::Package.manual_message().contains("package manager"));
        assert!(InstallKind::Flatpak.manual_message().contains("Flatpak"));
        assert_eq!(InstallKind::Package.manifest_key(), None);
    }
}
