//! Update checks and adding the app to the desktop's menus, on top of the host's [`AppServices`] (when it has them).
//!
//! The host does the work on its own threads. Here live the choices around it: what is remembered between runs (the automatic check, the
//! last check, a skipped version, "don't ask again"), when an automatic check is due, which dialog is up and what its buttons do.
//! Nothing here runs without a host that offers the services, and nothing leaves the machine unless the user asked or switched the
//! automatic check on.
use super::*;
use alloc::string::String;
use rvp_host::{Integration, UpdateHow, UpdateState};
use rvp_ui::{DialogButton, DialogSpec, DialogToggle};

/// Where the choices are kept.
pub const APP_SETTINGS_KEY: &str = "settings/app";
/// An automatic check is due after this long.
const CHECK_EVERY_SECS: i64 = 24 * 3600;

/// What the user said about adding the app to the app menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IntegrationChoice {
    /// Not asked yet (offer it).
    #[default]
    Ask,
    /// "Don't ask again".
    Never,
    /// It was added (or removed again on purpose, which is also an answer).
    Done,
}

/// What is kept between runs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppSettings {
    /// Check for updates by itself, at most once a day (off unless the user switches it on).
    pub auto_check: bool,
    /// When a check last started, Unix seconds (0 = never).
    pub last_check: i64,
    /// A version the user chose to skip: the automatic check does not announce it.
    pub skipped: String,
    /// The app-menu offer.
    pub integration: IntegrationChoice,
}

impl AppSettings {
    /// The text to keep.
    pub fn to_text(&self) -> String {
        format!(
            "rvp-app-settings 1\nauto_check={}\nlast_check={}\nskipped={}\nintegration={}\n",
            self.auto_check as u8,
            self.last_check,
            self.skipped.replace(['\n', '\r'], ""),
            match self.integration {
                IntegrationChoice::Ask => "ask",
                IntegrationChoice::Never => "never",
                IntegrationChoice::Done => "done",
            }
        )
    }

    /// Read kept text (anything unreadable keeps its default).
    pub fn from_text(text: &str) -> Option<AppSettings> {
        let mut lines = text.lines();
        if lines.next()?.trim() != "rvp-app-settings 1" {
            return None;
        }
        let mut s = AppSettings::default();
        for l in lines {
            let Some((k, v)) = l.split_once('=') else { continue };
            match k.trim() {
                "auto_check" => s.auto_check = v.trim() == "1",
                "last_check" => s.last_check = v.trim().parse().unwrap_or(0),
                "skipped" => s.skipped = v.trim().to_string(),
                "integration" => {
                    s.integration = match v.trim() {
                        "never" => IntegrationChoice::Never,
                        "done" => IntegrationChoice::Done,
                        _ => IntegrationChoice::Ask,
                    }
                }
                _ => {}
            }
        }
        Some(s)
    }
}

/// Which dialog is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    /// Update status, with the switches.
    Update,
    /// The first-run offer to add the app to the app menu.
    Integrate,
}

/// What a dialog button does (kept in step with the buttons of the spec on screen).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Btn {
    Check,
    Install,
    Cancel,
    Restart,
    Close,
    Skip,
    AddToMenu,
    NotNow,
    Never,
}

/// What a dialog switch does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tog {
    AutoCheck,
    AppMenu,
}

/// What the host last said (read every tick).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Cache {
    present: bool,
    version: String,
    updates: bool,
    state: UpdateState,
    integration: Integration,
    /// The version of the newest offer seen (the state forgets it while downloading).
    offer: String,
}

/// The services part of the app's state.
#[derive(Debug, Default)]
pub(crate) struct Services {
    loaded: bool,
    settings: AppSettings,
    screen: Option<Screen>,
    cache: Cache,
    /// The check in flight was started by the clock, not by the user.
    auto_pending: bool,
    /// An automatic check was started this run.
    auto_started: bool,
    /// The app-menu offer was made this run.
    offered: bool,
    /// The version the prompt was shown for this run ("Later" keeps it quiet until the next run).
    announced: String,
    buttons: Vec<Btn>,
    toggles: Vec<Tog>,
    /// A message for the dialog from the user's last action (an error from the host).
    notice: Option<String>,
}

fn percent(done: u64, total: u64) -> u16 {
    if total == 0 { 0 } else { ((done.min(total) as u128 * 1000) / total as u128) as u16 }
}

impl App {
    /// Read what the host says, load the kept choices once, run an automatic check when one is due, open a dialog when something wants
    /// attention.
    pub(crate) fn services_tick<H>(&mut self, host: &mut H)
    where
        H: Host<Video = FrameSink>,
    {
        let loaded = self.svc.loaded;
        let stored =
            if loaded { None } else { rvp_core::task::block_on(host.storage().load(APP_SETTINGS_KEY)) };
        let Some(svc) = host.app_services() else {
            self.svc.cache.present = false;
            self.svc.loaded = true;
            return;
        };
        let mut cache = Cache {
            present: true,
            version: svc.version(),
            updates: svc.updates_supported(),
            state: svc.update_state(),
            integration: svc.integration(),
            offer: self.svc.cache.offer.clone(),
        };
        if let UpdateState::Available { version, .. } | UpdateState::Ready { version } = &cache.state {
            cache.offer = version.clone();
        }
        let now_unix = svc.unix_time();
        let mut persist = false;
        let mut open_screen = None;
        if !self.svc.loaded {
            self.svc.loaded = true;
            if let Some(s) =
                stored.as_deref().and_then(|b| core::str::from_utf8(b).ok()).and_then(AppSettings::from_text)
            {
                self.svc.settings = s;
            }
        }
        // An automatic check, when switched on and a day has passed (or the clock is odd: never stay silent forever).
        let s = &mut self.svc;
        let due = s.settings.last_check == 0
            || s.settings.last_check > now_unix
            || now_unix - s.settings.last_check >= CHECK_EVERY_SECS;
        if s.settings.auto_check
            && cache.updates
            && !s.auto_started
            && cache.state == UpdateState::Idle
            && due
        {
            s.auto_started = true;
            s.auto_pending = true;
            s.settings.last_check = now_unix;
            persist = true;
            svc.check_updates();
            cache.state = UpdateState::Checking;
        }
        // What the automatic check found.
        if s.auto_pending {
            match &cache.state {
                UpdateState::Available { version, .. } => {
                    s.auto_pending = false;
                    if *version != s.settings.skipped && *version != s.announced {
                        s.announced = version.clone();
                        open_screen = Some(Screen::Update);
                    }
                }
                UpdateState::UpToDate | UpdateState::Failed(_) => {
                    s.auto_pending = false;
                    svc.reset_update();
                    cache.state = UpdateState::Idle;
                }
                _ => {}
            }
        }
        // An installed update asks for its restart, wherever the user was.
        if matches!(cache.state, UpdateState::Ready { .. }) && s.announced != "ready" {
            s.announced = "ready".into();
            if s.screen.is_none() {
                open_screen = Some(Screen::Update);
            }
        }
        // The first-run offer to add the app to the app menu.
        if !s.offered && cache.integration != Integration::Unavailable {
            s.offered = true;
            match (cache.integration, s.settings.integration) {
                (Integration::Off, IntegrationChoice::Ask) => {
                    if open_screen.is_none() && s.screen.is_none() {
                        open_screen = Some(Screen::Integrate);
                    }
                }
                (Integration::On, IntegrationChoice::Ask) => {
                    s.settings.integration = IntegrationChoice::Done;
                    persist = true;
                }
                _ => {}
            }
        }
        if let Some(screen) = open_screen {
            if s.screen.is_none() {
                s.screen = Some(screen);
            }
        }
        s.cache = cache;
        if persist {
            self.save_app_settings(host);
        }
    }

    fn save_app_settings<H>(&mut self, host: &mut H)
    where
        H: Host<Video = FrameSink>,
    {
        let text = self.svc.settings.to_text();
        rvp_core::task::block_on(host.storage().store(APP_SETTINGS_KEY, text.as_bytes()));
    }

    /// What is kept between runs about updates and the app menu.
    pub fn app_settings(&self) -> &AppSettings {
        &self.svc.settings
    }

    /// What the menus offer.
    pub(crate) fn app_model(&self) -> rvp_ui::AppModel {
        let c = &self.svc.cache;
        rvp_ui::AppModel {
            updates: c.present && c.updates,
            integration: match c.integration {
                Integration::Unavailable => None,
                Integration::Off => Some(false),
                Integration::On => Some(true),
            },
        }
    }

    /// The dialog on screen, if any (this also fixes what its buttons and switches mean).
    pub(crate) fn dialog_spec(&mut self) -> Option<DialogSpec> {
        let screen = self.svc.screen?;
        if !self.svc.cache.present {
            self.svc.screen = None;
            return None;
        }
        let c = self.svc.cache.clone();
        let mut spec = DialogSpec::default();
        let mut buttons: Vec<Btn> = Vec::new();
        let mut toggles: Vec<Tog> = Vec::new();
        match screen {
            Screen::Integrate => {
                spec.title = "Add Rusty Wave to the app menu?".into();
                match &self.svc.notice {
                    Some(n) => {
                        spec.body.push(n.clone());
                        spec.buttons.push(DialogButton::new("Close", true));
                        buttons.push(Btn::Close);
                    }
                    None => {
                        spec.body.push(
                            "Rusty Wave is running from an AppImage file. Add it to your applications menu to start it like any other app \
                             and to pick it for opening media files."
                                .into(),
                        );
                        spec.body.push("You can take it out again from the right-click menu.".into());
                        for (label, primary, kind) in [
                            ("Add to the app menu", true, Btn::AddToMenu),
                            ("Not now", false, Btn::NotNow),
                            ("Don't ask again", false, Btn::Never),
                        ] {
                            spec.buttons.push(DialogButton::new(label, primary));
                            buttons.push(kind);
                        }
                    }
                }
            }
            Screen::Update => {
                let ver = c.version.clone();
                spec.title = "Updates".into();
                match &c.state {
                    UpdateState::Idle => {
                        spec.body.push(format!("You are running Rusty Wave {ver}."));
                    }
                    UpdateState::Checking => spec.body.push("Checking for a newer version\u{2026}".into()),
                    UpdateState::UpToDate => {
                        spec.body.push(format!("Rusty Wave {ver} is the newest version."))
                    }
                    UpdateState::Managed(m) => {
                        spec.body.push(format!("You are running Rusty Wave {ver}."));
                        spec.body.push(m.clone());
                    }
                    UpdateState::Available { version, how } => {
                        spec.title = format!("Rusty Wave {version} is available");
                        match how {
                            UpdateHow::Install => {
                                spec.body.push(format!(
                                    "You have {ver}. The update is downloaded, checked against the release signature and installed in place."
                                ));
                            }
                            UpdateHow::Manual { message, .. } => {
                                spec.body.push(format!("You have {ver}."));
                                spec.body.push(message.clone());
                            }
                        }
                    }
                    UpdateState::Downloading { done, total } => {
                        spec.title = format!("Downloading Rusty Wave {}", c.offer);
                        spec.progress = Some(percent(*done, *total));
                        spec.body.push(format!("{}%", percent(*done, *total) / 10));
                    }
                    UpdateState::Installing => {
                        spec.title = format!("Installing Rusty Wave {}", c.offer);
                        spec.body
                            .push("Checking the signature and putting the update in place\u{2026}".into());
                    }
                    UpdateState::Ready { version } => {
                        spec.title = "Restart to finish updating".into();
                        spec.body
                            .push(format!("Rusty Wave {version} is installed. Restart to start using it."));
                    }
                    UpdateState::Failed(m) => {
                        spec.title = "Update problem".into();
                        spec.body.push(m.clone());
                    }
                }
                if let Some(n) = &self.svc.notice {
                    spec.body.push(n.clone());
                }
                let mut add = |label: &str, primary: bool, kind: Btn| {
                    spec.buttons.push(DialogButton::new(label, primary));
                    buttons.push(kind);
                };
                match &c.state {
                    UpdateState::Idle | UpdateState::UpToDate | UpdateState::Failed(_) => {
                        add("Check for updates", true, Btn::Check);
                        add("Close", false, Btn::Close);
                    }
                    UpdateState::Checking => add("Close", false, Btn::Close),
                    UpdateState::Managed(_) => add("Close", true, Btn::Close),
                    UpdateState::Available { how: UpdateHow::Install, .. } => {
                        add("Update now", true, Btn::Install);
                        add("Later", false, Btn::Close);
                        add("Skip this version", false, Btn::Skip);
                    }
                    UpdateState::Available { how: UpdateHow::Manual { .. }, .. } => {
                        add("Close", true, Btn::Close);
                        add("Skip this version", false, Btn::Skip);
                    }
                    UpdateState::Downloading { .. } => add("Cancel", false, Btn::Cancel),
                    UpdateState::Installing => {}
                    UpdateState::Ready { .. } => {
                        add("Restart now", true, Btn::Restart);
                        add("Later", false, Btn::Close);
                    }
                }
                let busy = matches!(c.state, UpdateState::Downloading { .. } | UpdateState::Installing);
                if !busy {
                    if c.updates {
                        spec.toggles.push(DialogToggle {
                            label: "Check for updates automatically".into(),
                            desc: "At most once a day. It only asks for the version file.".into(),
                            on: self.svc.settings.auto_check,
                        });
                        toggles.push(Tog::AutoCheck);
                    }
                    if c.integration != Integration::Unavailable {
                        spec.toggles.push(DialogToggle {
                            label: "Show in the app menu".into(),
                            desc: "A launcher entry and icons for this AppImage.".into(),
                            on: c.integration == Integration::On,
                        });
                        toggles.push(Tog::AppMenu);
                    }
                }
            }
        }
        self.svc.buttons = buttons;
        self.svc.toggles = toggles;
        Some(spec)
    }

    /// The menu entry: open the update dialog and look for a newer version.
    pub(crate) fn check_for_updates<H>(&mut self, host: &mut H, now: Timestamp)
    where
        H: Host<Video = FrameSink>,
    {
        let Some(svc) = host.app_services() else { return };
        if !svc.updates_supported() {
            return;
        }
        self.svc.notice = None;
        self.svc.screen = Some(Screen::Update);
        let state = svc.update_state();
        if !matches!(
            state,
            UpdateState::Downloading { .. }
                | UpdateState::Installing
                | UpdateState::Ready { .. }
                | UpdateState::Checking
        ) {
            self.svc.auto_pending = false;
            self.svc.settings.last_check = svc.unix_time();
            svc.check_updates();
            self.save_app_settings(host);
        }
        self.refresh_model(now);
    }

    /// The menu entry: add the app to the app menu, or take it out.
    pub(crate) fn toggle_integration<H>(&mut self, host: &mut H, now: Timestamp)
    where
        H: Host<Video = FrameSink>,
    {
        let Some(svc) = host.app_services() else { return };
        let on = match svc.integration() {
            Integration::Unavailable => return,
            Integration::On => false,
            Integration::Off => true,
        };
        match svc.set_integration(on) {
            Ok(()) => {
                // Either way the user has answered the offer.
                self.svc.settings.integration =
                    if on { IntegrationChoice::Done } else { IntegrationChoice::Never };
                self.save_app_settings(host);
                self.ui
                    .show_toast(if on { "Added to the app menu" } else { "Removed from the app menu" }, now);
            }
            Err(e) => self.ui.show_toast(&format!("Couldn't change the app menu: {e}"), now),
        }
    }

    /// A button of the dialog.
    pub(crate) fn dialog_button<H>(&mut self, host: &mut H, n: u8, now: Timestamp)
    where
        H: Host<Video = FrameSink>,
    {
        let Some(btn) = self.svc.buttons.get(n as usize).copied() else { return };
        let version = self.svc.cache.offer.clone();
        let Some(svc) = host.app_services() else { return };
        self.svc.notice = None;
        match btn {
            Btn::Check => {
                self.svc.auto_pending = false;
                self.svc.settings.last_check = svc.unix_time();
                svc.check_updates();
                self.save_app_settings(host);
            }
            Btn::Install => svc.install_update(),
            Btn::Cancel => svc.cancel_update(),
            Btn::Restart => {
                if let Err(e) = svc.restart() {
                    self.svc.notice = Some(format!("Couldn't restart: {e}"));
                }
                // On success the host quits.
            }
            Btn::Close => self.close_dialog(host),
            Btn::Skip => {
                self.svc.settings.skipped = version;
                self.save_app_settings(host);
                self.close_dialog(host);
                self.ui.show_toast("Skipped. You can still update from the right-click menu.", now);
            }
            Btn::AddToMenu => match svc.set_integration(true) {
                Ok(()) => {
                    self.svc.settings.integration = IntegrationChoice::Done;
                    self.save_app_settings(host);
                    self.close_dialog(host);
                    self.ui.show_toast("Added to the app menu", now);
                }
                Err(e) => self.svc.notice = Some(format!("Couldn't add it: {e}")),
            },
            Btn::NotNow => self.close_dialog(host),
            Btn::Never => {
                self.svc.settings.integration = IntegrationChoice::Never;
                self.save_app_settings(host);
                self.close_dialog(host);
                self.ui.show_toast("You can add it later from the right-click menu.", now);
            }
        }
        self.refresh_model(now);
    }

    /// A switch of the dialog.
    pub(crate) fn dialog_toggle<H>(&mut self, host: &mut H, n: u8, now: Timestamp)
    where
        H: Host<Video = FrameSink>,
    {
        let Some(tog) = self.svc.toggles.get(n as usize).copied() else { return };
        self.svc.notice = None;
        match tog {
            Tog::AutoCheck => {
                self.svc.settings.auto_check = !self.svc.settings.auto_check;
                self.save_app_settings(host);
            }
            Tog::AppMenu => {
                self.toggle_integration(host, now);
                // A failure shows as a toast; the switch follows what the host says next tick.
            }
        }
        self.refresh_model(now);
    }

    /// The dialog's X or Escape.
    pub(crate) fn dialog_close<H>(&mut self, host: &mut H, now: Timestamp)
    where
        H: Host<Video = FrameSink>,
    {
        self.close_dialog(host);
        self.refresh_model(now);
    }

    fn close_dialog<H>(&mut self, host: &mut H)
    where
        H: Host<Video = FrameSink>,
    {
        self.svc.screen = None;
        self.svc.notice = None;
        if matches!(self.svc.cache.state, UpdateState::Ready { .. }) {
            self.svc.announced = "ready".into();
        }
        // A failure or an "up to date" is not kept around for the next time.
        if let Some(svc) = host.app_services() {
            if matches!(svc.update_state(), UpdateState::Failed(_) | UpdateState::UpToDate) {
                svc.reset_update();
                self.svc.cache.state = UpdateState::Idle;
            }
        }
    }
}
