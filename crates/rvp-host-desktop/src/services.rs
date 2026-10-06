//! The desktop's [`AppServices`]: update checks and self-update through `rvp-update`, and adding the app to the desktop's menus
//! (an AppImage's launcher entry and icons on Linux; the Start menu and per-user file associations for the portable Windows build).
//!
//! The work runs on background threads; the app polls. A restart is *requested* here and carried out by the window loop after the
//! window has closed and its state is saved, so the new copy never starts beside a half-closed old one.
use rvp_host::{
    AppServices, DefaultOutcome, DefaultPlayer, Integration, UpdateHow, UpdateState, media_types_by_id,
};
use rvp_update::defaults;
use rvp_update::integrate_linux as linux;
#[cfg(windows)]
use rvp_update::integrate_windows as windows;
use rvp_update::{Config, Env, InstallKind, MANIFEST_URL, Net, Offer, State, Updater, Verifier, Version};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How the environment variable and the command line name another manifest (a `file://` URL or a path), for tests and for people who
/// host their own.
pub const MANIFEST_ENV: &str = "RVP_UPDATE_MANIFEST_URL";

/// What the app-menu integration does on this system.
enum Menu {
    /// Nothing to integrate with here (a package, Flatpak, a browser).
    None,
    /// An AppImage on Linux.
    AppImage { appimage: PathBuf, data_home: PathBuf },
    /// The portable Windows executable.
    WindowsPortable { exe: PathBuf },
}

/// The services of the desktop host.
pub struct DesktopServices {
    updater: Updater,
    kind: InstallKind,
    menu: Menu,
    /// The window loop reads this after each tick and quits when it is set.
    restart_requested: Rc<Cell<bool>>,
    /// Cached integration state and when it was read (the file system is looked at once a second at most).
    cached: Cell<Option<(Instant, Integration)>>,
    /// What to say instead of checking (Flatpak).
    managed: Option<String>,
    /// A check that was answered with `managed`.
    managed_shown: bool,
    version: String,
}

/// The manifest to use: the command line's, the environment's, else the published one.
pub fn manifest_url(cli: Option<&str>) -> String {
    cli.map(str::to_string)
        .or_else(|| std::env::var(MANIFEST_ENV).ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| MANIFEST_URL.to_string())
}

/// The update configuration of this running copy.
pub fn config(manifest: &str, restart_args: Vec<std::ffi::OsString>) -> Config {
    let env = Env::current();
    let kind = InstallKind::detect(&env, &|p: &Path| p.exists());
    let version = env!("CARGO_PKG_VERSION");
    let ua = format!("RustyWave/{version}");
    Config {
        manifest_url: manifest.to_string(),
        current: Version::parse(version).expect("the crate version is a version"),
        kind,
        verifier: Arc::new(Verifier::release()),
        source: Arc::new(Net::new(&ua)),
        temp_dir: std::env::temp_dir().join("rusty-wave-update"),
        restart_args,
    }
}

impl DesktopServices {
    /// Services for this copy of the program. Repairs a stale menu entry (the AppImage was moved) on the way.
    pub fn new(manifest: &str, restart_args: Vec<std::ffi::OsString>) -> DesktopServices {
        let cfg = config(manifest, restart_args);
        let kind = cfg.kind.clone();
        let menu = match &kind {
            InstallKind::AppImage(p) => match linux::data_home() {
                Some(h) => Menu::AppImage { appimage: p.clone(), data_home: h },
                None => Menu::None,
            },
            InstallKind::WindowsPortable(exe) => Menu::WindowsPortable { exe: exe.clone() },
            _ => Menu::None,
        };
        let managed = matches!(kind, InstallKind::Flatpak).then(|| {
            "Rusty Wave from Flatpak is updated through Flatpak or your software store.".to_string()
        });
        let s = DesktopServices {
            updater: Updater::new(cfg),
            kind,
            menu,
            restart_requested: Rc::new(Cell::new(false)),
            cached: Cell::new(None),
            managed,
            managed_shown: false,
            version: env!("CARGO_PKG_VERSION").to_string(),
        };
        s.repair_menu();
        s
    }

    /// Point a stale menu entry at where the program is now (only entries this program wrote).
    fn repair_menu(&self) {
        match &self.menu {
            Menu::AppImage { appimage, data_home } => {
                let _ = linux::repair(data_home, appimage, &linux::SystemRunner);
            }
            Menu::WindowsPortable { exe } => {
                #[cfg(windows)]
                {
                    let mut reg = windows::HkcuRegistry;
                    if matches!(windows::associations_status(&reg, exe), windows::Status::Stale(_)) {
                        let _ = windows::add_associations(&mut reg, exe);
                        if let Some(lnk) = windows::shortcut_path().filter(|l| l.exists()) {
                            let _ = windows::create_shortcut(&lnk, exe);
                        }
                        windows::notify_assoc_changed();
                    }
                }
                #[cfg(not(windows))]
                let _ = exe;
            }
            Menu::None => {}
        }
    }

    /// Set after "Restart now": the window loop quits, then [`DesktopServices::finish_restart`] runs.
    pub fn restart_flag(&self) -> Rc<Cell<bool>> {
        self.restart_requested.clone()
    }

    /// Start the new version (call after the window is gone). Returns what to print if that fails.
    pub fn finish_restart(&self) -> Result<(), String> {
        self.updater.restart().map_err(|e| e.to_string())
    }

    /// The updater (for the command line hook).
    pub fn updater(&self) -> &Updater {
        &self.updater
    }

    fn read_integration(&self) -> Integration {
        match &self.menu {
            Menu::None => Integration::Unavailable,
            Menu::AppImage { appimage, data_home } => match linux::status(data_home, appimage) {
                linux::Status::Absent => Integration::Off,
                linux::Status::Current | linux::Status::Stale(_) => Integration::On,
                linux::Status::Foreign => Integration::Unavailable,
            },
            Menu::WindowsPortable { exe } => {
                #[cfg(windows)]
                {
                    let reg = windows::HkcuRegistry;
                    let lnk_there = windows::shortcut_path().is_some_and(|l| l.exists());
                    if matches!(windows::associations_status(&reg, exe), windows::Status::Absent)
                        && !lnk_there
                    {
                        Integration::Off
                    } else {
                        Integration::On
                    }
                }
                #[cfg(not(windows))]
                {
                    let _ = exe;
                    Integration::Unavailable
                }
            }
        }
    }
}

impl AppServices for DesktopServices {
    fn version(&self) -> String {
        self.version.clone()
    }

    fn unix_time(&self) -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
    }

    fn updates_supported(&self) -> bool {
        true
    }

    fn update_state(&mut self) -> UpdateState {
        if self.managed_shown {
            return UpdateState::Managed(self.managed.clone().unwrap_or_default());
        }
        match self.updater.state() {
            State::Idle => UpdateState::Idle,
            State::Checking => UpdateState::Checking,
            State::UpToDate(_) => UpdateState::UpToDate,
            State::Available(Offer::Update { release, .. }) => {
                UpdateState::Available { version: release.version.to_string(), how: UpdateHow::Install }
            }
            State::Available(Offer::Manual { release, message, url }) => UpdateState::Available {
                version: release.version.to_string(),
                how: UpdateHow::Manual { message, url },
            },
            State::Downloading { done, total } => UpdateState::Downloading { done, total },
            State::Installing => UpdateState::Installing,
            State::Ready(release) => UpdateState::Ready { version: release.version.to_string() },
            State::Failed(m) => UpdateState::Failed(m),
        }
    }

    fn check_updates(&mut self) {
        if self.managed.is_some() {
            // Nothing to ask the network (the sandbox has none): say where updates come from.
            self.managed_shown = true;
            return;
        }
        self.updater.check();
    }

    fn install_update(&mut self) {
        self.updater.install();
    }

    fn cancel_update(&mut self) {
        self.updater.cancel();
    }

    fn reset_update(&mut self) {
        self.managed_shown = false;
        self.updater.reset();
    }

    fn restart(&mut self) -> Result<(), String> {
        // Better to say no now than after the window is gone.
        if let State::Ready(_) = self.updater.state() {
            self.restart_requested.set(true);
            Ok(())
        } else {
            Err("there is no installed update to start".into())
        }
    }

    fn integration(&mut self) -> Integration {
        if let Some((at, v)) = self.cached.get()
            && at.elapsed() < Duration::from_secs(1)
        {
            return v;
        }
        let v = self.read_integration();
        self.cached.set(Some((Instant::now(), v)));
        v
    }

    fn offers_enabled(&self) -> bool {
        // Scripted runs (the smoke tests, kiosks) switch the first-run offers off.
        std::env::var_os("RVP_NO_OFFERS").is_none_or(|v| v.is_empty() || v == "0")
    }

    fn default_player(&mut self) -> DefaultPlayer {
        match &self.kind {
            InstallKind::WindowsInstaller | InstallKind::WindowsPortable(_) => DefaultPlayer::Available {
                note: "Windows does not let a program take over the defaults by itself. Rusty Wave registers itself and opens \
                       Windows\u{2019} Default apps page; choose Rusty Wave there for the kinds of files you want."
                    .into(),
                silent: false,
            },
            InstallKind::MacApp => DefaultPlayer::Available {
                note: "macOS may ask you to confirm the change.".into(),
                silent: true,
            },
            InstallKind::AppImage(_) => DefaultPlayer::Available {
                note: if matches!(self.read_integration(), Integration::On) {
                    String::new()
                } else {
                    "Rusty Wave is added to the app menu first, so the system can find it.".into()
                },
                silent: true,
            },
            InstallKind::Package | InstallKind::Flatpak | InstallKind::Other if cfg!(target_os = "linux") => {
                DefaultPlayer::Available { note: String::new(), silent: true }
            }
            _ => DefaultPlayer::Unavailable,
        }
    }

    fn set_default_player(&mut self, type_ids: &[String]) -> Result<DefaultOutcome, String> {
        let ids: Vec<&str> = type_ids.iter().map(String::as_str).collect();
        let types = media_types_by_id(&ids);
        if types.is_empty() {
            return Err("no kind of file was chosen".into());
        }
        match &self.kind {
            InstallKind::WindowsInstaller | InstallKind::WindowsPortable(_) => {
                let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                register_windows(&exe).map_err(|e| e.to_string())?;
                defaults::open_windows_default_apps().map_err(|e| e.to_string())?;
                Ok(DefaultOutcome::UserMustConfirm(
                    "Rusty Wave is registered, and Windows\u{2019} Default apps page is open. Choose Rusty Wave for the kinds of files you ticked \
                     (Windows keeps the final say). You can close this."
                        .into(),
                ))
            }
            InstallKind::MacApp => {
                let exts: Vec<&str> = types.iter().flat_map(|t| t.extensions.iter().copied()).collect();
                defaults::set_macos_defaults(defaults::MACOS_BUNDLE_ID, &exts)
                    .map(|_| DefaultOutcome::Set(types.len()))
                    .map_err(|e| e.to_string())
            }
            _ => {
                // The desktop entry has to exist for the system to take it as a handler: add the app menu entry of an AppImage first.
                if matches!(self.menu, Menu::AppImage { .. }) && self.read_integration() != Integration::On {
                    self.set_integration(true)?;
                }
                let mimes: Vec<&str> = types.iter().flat_map(|t| t.mimes.iter().copied()).collect();
                let home = defaults::config_home().ok_or("cannot find the configuration folder")?;
                defaults::set_linux_defaults(&home, defaults::DESKTOP_ID, &mimes)
                    .map_err(|e| e.to_string())?;
                Ok(DefaultOutcome::Set(types.len()))
            }
        }
    }

    fn set_integration(&mut self, on: bool) -> Result<(), String> {
        self.cached.set(None);
        let r: std::io::Result<()> = match &self.menu {
            Menu::None => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "this installation has no menu entry to change",
            )),
            Menu::AppImage { appimage, data_home } => {
                if on {
                    linux::add(data_home, appimage, &linux::SystemRunner)
                } else {
                    linux::remove(data_home, &linux::SystemRunner)
                }
            }
            Menu::WindowsPortable { exe } => set_windows(on, exe),
        };
        r.map_err(|e| e.to_string())
    }
}

#[cfg(windows)]
fn set_windows(on: bool, exe: &Path) -> std::io::Result<()> {
    let mut reg = windows::HkcuRegistry;
    let lnk = windows::shortcut_path();
    let r = if on {
        let added = windows::add_associations(&mut reg, exe).and_then(|()| match &lnk {
            Some(l) => windows::create_shortcut(l, exe),
            None => Ok(()),
        });
        if added.is_err() {
            // All or nothing: a half-added entry would read as "in the menu" without being usable.
            let _ = windows::remove_associations(&mut reg);
        }
        added
    } else {
        windows::remove_associations(&mut reg)
            .and_then(|()| lnk.as_deref().map_or(Ok(()), windows::remove_shortcut))
    };
    windows::notify_assoc_changed();
    r
}

/// The file associations (and nothing else) for the current user, so Windows lists the app in *Default apps*.
#[cfg(windows)]
fn register_windows(exe: &Path) -> std::io::Result<()> {
    let mut reg = windows::HkcuRegistry;
    windows::add_associations(&mut reg, exe)?;
    windows::notify_assoc_changed();
    Ok(())
}

#[cfg(not(windows))]
fn register_windows(_exe: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "this is not Windows"))
}

#[cfg(not(windows))]
fn set_windows(_on: bool, _exe: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "this is not Windows"))
}

/// A short description of how this copy is installed, for `--version`-style diagnostics.
pub fn describe(kind: &InstallKind) -> &'static str {
    match kind {
        InstallKind::AppImage(_) => "AppImage",
        InstallKind::Flatpak => "Flatpak",
        InstallKind::Package => "system package",
        InstallKind::WindowsInstaller => "Windows installer",
        InstallKind::WindowsPortable(_) => "portable",
        InstallKind::MacApp => "macOS app",
        InstallKind::Other => "other",
    }
}

impl DesktopServices {
    /// How this copy is installed.
    pub fn kind(&self) -> &InstallKind {
        &self.kind
    }
}
