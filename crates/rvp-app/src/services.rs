//! Update checks and adding the app to the desktop's menus, on top of the host's [`AppServices`] (when it has them).
//!
//! The host does the work on its own threads. Here live the choices around it: what is remembered between runs (the automatic check, the
//! last check, a skipped version, "don't ask again"), when an automatic check is due, which dialog is up and what its buttons do.
//! Nothing here runs without a host that offers the services, and nothing leaves the machine unless the user asked or switched the
//! automatic check on.
use super::*;
use alloc::string::String;
use rvp_host::{DefaultOutcome, DefaultPlayer, Integration, MEDIA_TYPES, UpdateHow, UpdateState};
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
    /// The default-media-player offer (`Ask` until the user answered it; `Never` after a "no thanks").
    pub default_player: IntegrationChoice,
}

fn choice_name(c: IntegrationChoice) -> &'static str {
    match c {
        IntegrationChoice::Ask => "ask",
        IntegrationChoice::Never => "never",
        IntegrationChoice::Done => "done",
    }
}

fn choice_from(s: &str) -> IntegrationChoice {
    match s {
        "never" => IntegrationChoice::Never,
        "done" => IntegrationChoice::Done,
        _ => IntegrationChoice::Ask,
    }
}

impl AppSettings {
    /// The text to keep.
    pub fn to_text(&self) -> String {
        format!(
            "rvp-app-settings 1\nauto_check={}\nlast_check={}\nskipped={}\nintegration={}\ndefault_player={}\n",
            self.auto_check as u8,
            self.last_check,
            self.skipped.replace(['\n', '\r'], ""),
            choice_name(self.integration),
            choice_name(self.default_player),
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
                "integration" => s.integration = choice_from(v.trim()),
                "default_player" => s.default_player = choice_from(v.trim()),
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
    /// Everything the user can set: theme, default player, app menu, updates, audio.
    Settings,
    /// Paste a link or CSS, preview a theme, apply it or go back to Unicorn Tears.
    Theme,
    /// "Set as default media player": the checklist of media types (`true` for the first-run offer, which can be declined for good).
    Default(bool),
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
    Never,
    // The Settings dialog.
    OpenAudio,
    OpenTheme,
    OpenDefault,
    ToggleMenu,
    OpenUpdates,
    // The default-player dialog.
    SetDefault,
    CheckAll,
    BackToSettings,
    // The Theme dialog.
    ThemePreview,
    ThemeApply,
    ThemeReset,
}

/// What a dialog switch does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tog {
    AutoCheck,
    AppMenu,
    /// Media type `n` of [`MEDIA_TYPES`] in the default-player checklist.
    Type(usize),
}

/// What the host last said (read every tick).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Cache {
    present: bool,
    version: String,
    updates: bool,
    state: UpdateState,
    integration: Integration,
    default_player: DefaultPlayer,
    offers: bool,
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
    /// The default-player offer was made this run.
    default_offered: bool,
    /// Which media types of the default-player checklist are ticked (all, until the user unticks).
    default_checked: Vec<bool>,
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
            default_player: svc.default_player(),
            offers: svc.offers_enabled(),
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
        // The first-run offer to make the app the default media player: once the menu offer (if any) is out of the way, and only
        // where the system lets an app do it.
        if (s.offered || cache.integration == Integration::Unavailable)
            && !s.default_offered
            && s.screen.is_none()
            && cache.offers
            && matches!(cache.default_player, DefaultPlayer::Available { .. })
        {
            s.default_offered = true;
            if s.settings.default_player == IntegrationChoice::Ask {
                s.default_checked = alloc::vec![true; MEDIA_TYPES.len()];
                s.screen = Some(Screen::Default(true));
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
            links: self.links,
        }
    }

    /// The dialog on screen, if any (this also fixes what its buttons and switches mean).
    pub(crate) fn dialog_spec(&mut self) -> Option<DialogSpec> {
        let screen = self.svc.screen?;
        // Settings and what hangs off it work without the host's services (the audio and theme parts do); the rest needs them.
        if !self.svc.cache.present && !matches!(screen, Screen::Settings | Screen::Theme) {
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
                        spec.body.push("You can add or remove it later in Settings.".into());
                        for (label, primary, kind) in
                            [("Add to the app menu", true, Btn::AddToMenu), ("No thanks", false, Btn::Never)]
                        {
                            spec.buttons.push(DialogButton::new(label, primary));
                            buttons.push(kind);
                        }
                    }
                }
            }
            Screen::Settings => {
                spec.title = "Settings".into();
                let ver = if c.version.is_empty() {
                    env!("CARGO_PKG_VERSION").to_string()
                } else {
                    c.version.clone()
                };
                spec.body.push(format!("Rusty Wave {ver}"));
                if let Some(n) = &self.svc.notice {
                    spec.body.push(n.clone());
                }
                let mut add = |label: &str, primary: bool, kind: Btn| {
                    spec.buttons.push(DialogButton::new(label, primary));
                    buttons.push(kind);
                };
                add("Audio settings\u{2026}", false, Btn::OpenAudio);
                add("Theme\u{2026}", false, Btn::OpenTheme);
                if matches!(c.default_player, DefaultPlayer::Available { .. }) {
                    add("Set as default media player\u{2026}", false, Btn::OpenDefault);
                }
                match c.integration {
                    Integration::Unavailable => {}
                    Integration::Off => add("Add to app menu", false, Btn::ToggleMenu),
                    Integration::On => add("Remove from app menu", false, Btn::ToggleMenu),
                }
                if c.updates {
                    add("Check for updates\u{2026}", false, Btn::OpenUpdates);
                }
                add("Close", true, Btn::Close);
            }
            Screen::Theme => {
                spec.title = "Theme".into();
                spec.body = self.themeui.body(&self.themeui.active_name());
                spec.input = Some(rvp_ui::dialog::DialogInput {
                    label: "Link or CSS".into(),
                    text: self.themeui.shown(),
                    placeholder: "https://\u{2026} or :root { --background: #\u{2026} }".into(),
                });
                let busy = self.themeui.busy();
                let has = self.themeui.has_candidate();
                let mut preview = DialogButton::new(if busy { "Fetching\u{2026}" } else { "Preview" }, !has);
                preview.enabled = !busy;
                spec.buttons.push(preview);
                buttons.push(Btn::ThemePreview);
                let mut apply = DialogButton::new("Apply", has);
                apply.enabled = has;
                spec.buttons.push(apply);
                buttons.push(Btn::ThemeApply);
                spec.buttons.push(DialogButton::new("Reset to Unicorn Tears", false));
                buttons.push(Btn::ThemeReset);
                spec.buttons.push(DialogButton::new("Close", false));
                buttons.push(Btn::Close);
            }
            Screen::Default(offer) => {
                spec.compact = true;
                let (note, silent) = match &c.default_player {
                    DefaultPlayer::Available { note, silent } => (note.clone(), *silent),
                    DefaultPlayer::Unavailable => (String::new(), false),
                };
                spec.title = if offer {
                    "Make Rusty Wave your default media player?".into()
                } else {
                    "Set as default media player".into()
                };
                match &self.svc.notice {
                    Some(n) => {
                        spec.body.push(n.clone());
                        spec.buttons.push(DialogButton::new("Close", true));
                        buttons.push(Btn::Close);
                    }
                    None => {
                        spec.body.push("Choose the kinds of files Rusty Wave should open. Everything is ticked to start with.".into());
                        if !note.is_empty() {
                            spec.body.push(note);
                        }
                        if self.svc.default_checked.len() != MEDIA_TYPES.len() {
                            self.svc.default_checked = alloc::vec![true; MEDIA_TYPES.len()];
                        }
                        for (i, t) in MEDIA_TYPES.iter().enumerate() {
                            spec.toggles.push(DialogToggle {
                                label: t.label.into(),
                                desc: String::new(),
                                on: self.svc.default_checked[i],
                            });
                            toggles.push(Tog::Type(i));
                        }
                        let any = self.svc.default_checked.iter().any(|&b| b);
                        let all = self.svc.default_checked.iter().all(|&b| b);
                        let mut b =
                            DialogButton::new(if silent { "Set as default" } else { "Continue" }, true);
                        b.enabled = any;
                        spec.buttons.push(b);
                        buttons.push(Btn::SetDefault);
                        spec.buttons
                            .push(DialogButton::new(if all { "Uncheck all" } else { "Check all" }, false));
                        buttons.push(Btn::CheckAll);
                        if offer {
                            spec.buttons.push(DialogButton::new("No thanks", false));
                            buttons.push(Btn::Never);
                        } else {
                            spec.buttons.push(DialogButton::new("Back", false));
                            buttons.push(Btn::BackToSettings);
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

    /// The menu entry and Ctrl+,: the Settings dialog.
    pub(crate) fn show_settings(&mut self, now: Timestamp) {
        self.svc.notice = None;
        self.svc.screen = Some(Screen::Settings);
        self.refresh_model(now);
    }

    /// The Theme dialog (see `theme_ui.rs`).
    fn open_theme(&mut self) {
        self.themeui.theme_opened_state();
        self.svc.screen = Some(Screen::Theme);
    }

    /// The dialog on screen has a text box that takes typing and pasting.
    pub(crate) fn dialog_takes_text(&self) -> bool {
        self.svc.screen == Some(Screen::Theme)
    }

    pub(crate) fn dialog_char(&mut self, c: char, now: Timestamp) {
        if self.dialog_takes_text() {
            self.theme_char(c);
            self.refresh_model(now);
        }
    }

    pub(crate) fn dialog_backspace(&mut self, now: Timestamp) {
        if self.dialog_takes_text() {
            self.theme_backspace();
            self.refresh_model(now);
        }
    }

    pub(crate) fn dialog_paste(&mut self, text: &str, now: Timestamp) {
        if self.dialog_takes_text() {
            self.theme_paste(text);
            self.refresh_model(now);
        }
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
        // What needs nothing from the host's services (the Settings dialog also opens in a browser).
        match btn {
            Btn::Close => {
                self.close_dialog(host);
                self.refresh_model(now);
                return;
            }
            Btn::OpenAudio => {
                self.svc.screen = None;
                self.ui.open_audio_settings();
                self.refresh_model(now);
                return;
            }
            Btn::OpenTheme => {
                self.svc.notice = None;
                self.open_theme();
                self.refresh_model(now);
                return;
            }
            Btn::ThemePreview => {
                self.theme_preview(host);
                self.refresh_model(now);
                return;
            }
            Btn::ThemeApply => {
                self.theme_apply(host, now);
                self.refresh_model(now);
                return;
            }
            Btn::ThemeReset => {
                self.theme_reset(host);
                self.refresh_model(now);
                return;
            }
            _ => {}
        }
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
            Btn::Never => {
                // A "no thanks" to either offer is final: it is not asked again, the Settings dialog has the button.
                match self.svc.screen {
                    Some(Screen::Default(_)) => {
                        self.svc.settings.default_player = IntegrationChoice::Never;
                        self.save_app_settings(host);
                        self.close_dialog(host);
                        self.ui.show_toast("You can set it later in Settings.", now);
                    }
                    _ => {
                        self.svc.settings.integration = IntegrationChoice::Never;
                        self.save_app_settings(host);
                        self.close_dialog(host);
                        self.ui.show_toast("You can add it later in Settings.", now);
                    }
                }
            }
            Btn::OpenAudio | Btn::OpenTheme | Btn::ThemePreview | Btn::ThemeApply | Btn::ThemeReset => {}
            Btn::OpenDefault => {
                self.svc.default_checked = alloc::vec![true; MEDIA_TYPES.len()];
                self.svc.screen = Some(Screen::Default(false));
            }
            Btn::BackToSettings => self.svc.screen = Some(Screen::Settings),
            Btn::ToggleMenu => {
                self.toggle_integration(host, now);
            }
            Btn::OpenUpdates => {
                self.check_for_updates(host, now);
            }
            Btn::CheckAll => {
                let all = self.svc.default_checked.iter().all(|&b| b);
                self.svc.default_checked = alloc::vec![!all; MEDIA_TYPES.len()];
            }
            Btn::SetDefault => {
                let ids: Vec<String> = MEDIA_TYPES
                    .iter()
                    .zip(self.svc.default_checked.iter())
                    .filter(|(_, on)| **on)
                    .map(|(t, _)| t.id.to_string())
                    .collect();
                match svc.set_default_player(&ids) {
                    Ok(DefaultOutcome::Set(n)) => {
                        self.svc.settings.default_player = IntegrationChoice::Done;
                        self.save_app_settings(host);
                        self.close_dialog(host);
                        self.ui.show_toast(
                            &format!(
                                "Rusty Wave now opens {}",
                                plural(n.max(ids.len()), "kind of file", "kinds of files")
                            ),
                            now,
                        );
                    }
                    Ok(DefaultOutcome::UserMustConfirm(text)) => {
                        self.svc.settings.default_player = IntegrationChoice::Done;
                        self.save_app_settings(host);
                        self.svc.notice = Some(text);
                    }
                    Err(e) => self.svc.notice = Some(format!("Couldn't set it: {e}")),
                }
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
            Tog::Type(i) => {
                if let Some(b) = self.svc.default_checked.get_mut(i) {
                    *b = !*b;
                }
            }
        }
        self.refresh_model(now);
    }

    /// The dialog's X or Escape.
    pub(crate) fn dialog_close<H>(&mut self, host: &mut H, now: Timestamp)
    where
        H: Host<Video = FrameSink>,
    {
        // Closing a first-run offer without answering is a no: it is not asked again (Settings has the buttons).
        match self.svc.screen {
            Some(Screen::Integrate) if self.svc.notice.is_none() => {
                self.svc.settings.integration = IntegrationChoice::Never;
                self.save_app_settings(host);
            }
            Some(Screen::Default(true)) if self.svc.notice.is_none() => {
                self.svc.settings.default_player = IntegrationChoice::Never;
                self.save_app_settings(host);
            }
            // Theme: an unapplied preview goes away; Settings is where it came from.
            Some(Screen::Theme) => {
                self.theme_closed();
                self.svc.screen = Some(Screen::Settings);
                self.refresh_model(now);
                return;
            }
            // The step-by-step dialogs of Settings go back to it; Settings itself closes.
            Some(Screen::Default(false)) if self.svc.notice.is_none() => {
                self.svc.screen = Some(Screen::Settings);
                self.refresh_model(now);
                return;
            }
            _ => {}
        }
        self.close_dialog(host);
        self.refresh_model(now);
    }

    fn close_dialog<H>(&mut self, host: &mut H)
    where
        H: Host<Video = FrameSink>,
    {
        if self.svc.screen == Some(Screen::Theme) {
            self.theme_closed();
        }
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
