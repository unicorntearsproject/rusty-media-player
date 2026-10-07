//! Update checks and the app-menu offer, through the whole app on the headless host with scripted services: what the menus offer, the
//! dialog for every state of an update, what is remembered (automatic check, skipped version, "don't ask again") across runs, the
//! once-a-day rule, and that nothing happens (and nothing is shown) on a host without the services. No media files are needed.
use rvp_app::{APP_SETTINGS_KEY, App, IntegrationChoice};
use rvp_host::{
    DefaultOutcome, DefaultPlayer, HostClock, InputEvent, Integration, Key, MEDIA_TYPES, Modifiers,
    PointerButton, ScriptedServices, UpdateHow, UpdateState,
};
use rvp_host_headless::{DefaultCodecs, UiHost};
use rvp_ui::UiConfig;
use rvp_ui::actions::{Action, context_menu};
use std::rc::Rc;

const DAY: i64 = 24 * 3600;

fn services(version: &str, updates: bool, integration: Integration) -> ScriptedServices {
    ScriptedServices {
        version: version.into(),
        now: 1_800_000_000,
        updates,
        integration,
        ..ScriptedServices::default()
    }
}

fn new_app() -> App {
    App::new(Rc::new(DefaultCodecs { stall: None, clock: None }), UiConfig { reduce_motion: true })
}

struct Rig {
    host: UiHost,
    app: App,
}

impl Rig {
    fn with(svc: Option<ScriptedServices>) -> Rig {
        let mut host = UiHost::new();
        host.services = svc;
        let mut r = Rig { host, app: new_app() };
        r.run(100);
        r
    }

    fn run(&mut self, ms: i64) {
        let clock = self.host.virtual_clock();
        let end = clock.now_us() + ms * 1000;
        while clock.now_us() < end {
            self.app.tick(&mut self.host);
            clock.advance(16_000);
        }
    }

    /// A new run of the program: the same saved settings, a new app.
    fn restart(&mut self, svc: ScriptedServices) {
        self.app = new_app();
        self.host.services = Some(svc);
        self.run(100);
    }

    fn svc(&mut self) -> &mut ScriptedServices {
        self.host.services.as_mut().unwrap()
    }

    fn do_action(&mut self, a: Action) {
        let now = self.host.virtual_clock().now_us();
        self.app.apply(&mut self.host, a, now);
        self.run(50);
    }

    fn send(&mut self, ev: InputEvent) {
        self.host.input.0.push_back(ev);
        self.app.pump(&mut self.host);
        self.run(50);
    }

    fn key(&mut self, k: Key) {
        self.send(InputEvent::KeyDown { key: k, mods: Modifiers::default(), repeat: false });
    }

    fn click(&mut self, x: f32, y: f32) {
        self.send(InputEvent::PointerMove { x, y });
        self.send(InputEvent::PointerDown { x, y, button: PointerButton::Primary });
        self.send(InputEvent::PointerUp { x, y, button: PointerButton::Primary });
    }

    fn title(&self) -> Option<String> {
        self.app.model().dialog.as_ref().map(|d| d.title.clone())
    }

    fn buttons(&self) -> Vec<String> {
        self.app
            .model()
            .dialog
            .as_ref()
            .map(|d| d.buttons.iter().map(|b| b.label.clone()).collect())
            .unwrap_or_default()
    }

    fn body(&self) -> String {
        self.app.model().dialog.as_ref().map(|d| d.body.join(" | ")).unwrap_or_default()
    }

    fn settings(&self) -> rvp_app::AppSettings {
        self.app.app_settings().clone()
    }

    fn saved(&self) -> Option<String> {
        self.host.storage.0.get(APP_SETTINGS_KEY).map(|b| String::from_utf8_lossy(b).into_owned())
    }

    fn press(&mut self, label: &str) {
        let i = self
            .buttons()
            .iter()
            .position(|b| b == label)
            .unwrap_or_else(|| panic!("no button {label}: {:?}", self.buttons()));
        self.do_action(Action::DialogButton(i as u8));
    }
}

fn available(version: &str, how: UpdateHow) -> UpdateState {
    UpdateState::Available { version: version.into(), how }
}

#[test]
fn a_host_without_the_services_shows_nothing_and_ignores_the_actions() {
    let mut r = Rig::with(None);
    let labels: Vec<String> = context_menu(r.app.model()).iter().map(|m| m.label.clone()).collect();
    assert!(!labels.iter().any(|l| l.contains("update") || l.contains("app menu")), "{labels:?}");
    r.do_action(Action::CheckForUpdates);
    r.do_action(Action::ToggleIntegration);
    r.do_action(Action::DialogButton(0));
    assert!(r.app.model().dialog.is_none());
    assert_eq!(r.saved(), None);
}

#[test]
fn the_menu_offers_only_what_the_host_offers() {
    let menu =
        |r: &Rig| -> Vec<String> { context_menu(r.app.model()).iter().map(|m| m.label.clone()).collect() };
    let r = Rig::with(Some(services("0.0.2", true, Integration::Off)));
    assert!(menu(&r).contains(&"Check for updates\u{2026}".to_string()));
    assert!(menu(&r).contains(&"Add to the app menu".to_string()));
    let r = Rig::with(Some(services("0.0.2", true, Integration::On)));
    assert!(menu(&r).contains(&"Remove from the app menu".to_string()));
    let r = Rig::with(Some(services("0.0.2", true, Integration::Unavailable)));
    assert!(menu(&r).contains(&"Check for updates\u{2026}".to_string()));
    assert!(!menu(&r).iter().any(|l| l.contains("app menu")));
    let r = Rig::with(Some(services("0.0.2", false, Integration::Unavailable)));
    assert!(!menu(&r).iter().any(|l| l.contains("update")));
}

#[test]
fn checking_by_hand_walks_through_every_state() {
    let mut r = Rig::with(Some(services("0.0.2", true, Integration::Unavailable)));
    assert!(r.title().is_none());
    r.do_action(Action::CheckForUpdates);
    assert_eq!(r.svc().calls, ["check"]);
    assert_eq!(r.title().as_deref(), Some("Updates"));
    assert!(r.body().contains("Checking"), "{}", r.body());
    assert_eq!(r.buttons(), ["Close"]);

    r.svc().state = UpdateState::UpToDate;
    r.run(50);
    assert!(r.body().contains("0.0.2 is the newest version"), "{}", r.body());
    assert_eq!(r.buttons(), ["Check for updates", "Close"]);

    r.svc().state = available("0.0.3", UpdateHow::Install);
    r.run(50);
    assert_eq!(r.title().as_deref(), Some("Rusty Wave 0.0.3 is available"));
    assert!(r.body().contains("You have 0.0.2"));
    assert_eq!(r.buttons(), ["Update now", "Later", "Skip this version"]);

    r.press("Update now");
    assert_eq!(r.svc().calls.last().unwrap(), "install");
    r.svc().state = UpdateState::Downloading { done: 250, total: 1000 };
    r.run(50);
    assert!(r.title().unwrap().starts_with("Downloading Rusty Wave 0.0.3"));
    assert_eq!(r.app.model().dialog.as_ref().unwrap().progress, Some(250));
    assert_eq!(r.buttons(), ["Cancel"]);
    r.press("Cancel");
    assert_eq!(r.svc().calls.last().unwrap(), "cancel");

    r.svc().state = UpdateState::Installing;
    r.run(50);
    assert!(r.buttons().is_empty());
    assert!(r.title().unwrap().starts_with("Installing"));

    r.svc().state = UpdateState::Ready { version: "0.0.3".into() };
    r.run(50);
    assert_eq!(r.title().as_deref(), Some("Restart to finish updating"));
    assert_eq!(r.buttons(), ["Restart now", "Later"]);
    r.svc().restart_error = Some("no permission".into());
    r.press("Restart now");
    assert!(r.body().contains("Couldn't restart: no permission"), "{}", r.body());
    r.svc().restart_error = None;
    r.press("Restart now");
    assert_eq!(r.svc().calls.iter().filter(|c| *c == "restart").count(), 2);

    r.press("Later");
    assert!(r.title().is_none());
}

#[test]
fn a_failure_is_said_in_words_and_closing_forgets_it() {
    let mut r = Rig::with(Some(services("0.0.2", true, Integration::Unavailable)));
    r.do_action(Action::CheckForUpdates);
    r.svc().state = UpdateState::Failed("Could not check for updates: cannot reach the server".into());
    r.run(50);
    assert_eq!(r.title().as_deref(), Some("Update problem"));
    assert!(r.body().contains("cannot reach the server"));
    assert_eq!(r.buttons(), ["Check for updates", "Close"]);
    r.press("Close");
    assert!(r.title().is_none());
    assert_eq!(r.svc().calls.last().unwrap(), "reset");
    // Trying again from the menu.
    r.do_action(Action::CheckForUpdates);
    assert_eq!(r.svc().calls.iter().filter(|c| *c == "check").count(), 2);
}

#[test]
fn updates_a_package_manager_owns_get_the_message_not_a_download() {
    let mut r = Rig::with(Some(services("0.0.2", true, Integration::Unavailable)));
    r.do_action(Action::CheckForUpdates);
    r.svc().state = available(
        "0.0.3",
        UpdateHow::Manual {
            message: "Update through your package manager (apt, dnf or your software centre).".into(),
            url: None,
        },
    );
    r.run(50);
    assert!(r.body().contains("package manager"));
    assert_eq!(r.buttons(), ["Close", "Skip this version"]);
}

#[test]
fn the_automatic_check_is_off_by_default_and_costs_nothing() {
    let mut r = Rig::with(Some(services("0.0.2", true, Integration::Unavailable)));
    r.run(500);
    assert!(r.svc().calls.is_empty());
    assert!(!r.settings().auto_check);
    // The switch is in the dialog and is remembered.
    r.do_action(Action::CheckForUpdates);
    r.svc().state = UpdateState::UpToDate;
    r.run(50);
    assert_eq!(r.app.model().dialog.as_ref().unwrap().toggles[0].label, "Check for updates automatically");
    r.do_action(Action::DialogToggle(0));
    assert!(r.settings().auto_check);
    assert!(r.saved().unwrap().contains("auto_check=1"));
    assert!(r.app.model().dialog.as_ref().unwrap().toggles[0].on);
}

#[test]
fn the_automatic_check_runs_at_most_once_a_day_and_prompts_only_for_news() {
    let mut r = Rig::with(Some(services("0.0.2", true, Integration::Unavailable)));
    r.do_action(Action::CheckForUpdates);
    r.svc().state = UpdateState::UpToDate;
    r.run(50);
    r.do_action(Action::DialogToggle(0)); // automatic on
    r.press("Close");
    let t0 = r.svc().now;

    // A new run, the same day: the manual check just now counts, so no automatic one.
    let mut s = services("0.0.2", true, Integration::Unavailable);
    s.now = t0 + 3600;
    r.restart(s);
    assert!(r.svc().calls.is_empty());

    // Next day: it runs by itself, and with nothing new it is silent and forgets the answer.
    let mut s = services("0.0.2", true, Integration::Unavailable);
    s.now = t0 + DAY + 5;
    s.check_result = Some(UpdateState::UpToDate);
    r.restart(s);
    r.run(50);
    assert_eq!(r.svc().calls, ["check", "reset"]);
    assert!(r.title().is_none());
    assert_eq!(r.settings().last_check, t0 + DAY + 5);
    r.run(500);
    assert_eq!(r.svc().calls, ["check", "reset"], "once per run");

    // A failure (offline) is silent too.
    let mut s = services("0.0.2", true, Integration::Unavailable);
    s.now = t0 + 3 * DAY;
    s.check_result = Some(UpdateState::Failed("offline".into()));
    r.restart(s);
    assert!(r.title().is_none());
    assert_eq!(r.svc().calls, ["check", "reset"]);

    // News: the prompt opens by itself, "Skip this version" is remembered and the next day's check stays quiet about it.
    let mut s = services("0.0.2", true, Integration::Unavailable);
    s.now = t0 + 5 * DAY;
    s.check_result = Some(available("0.0.3", UpdateHow::Install));
    r.restart(s);
    assert_eq!(r.title().as_deref(), Some("Rusty Wave 0.0.3 is available"));
    r.press("Skip this version");
    assert!(r.title().is_none());
    assert_eq!(r.settings().skipped, "0.0.3");
    assert!(r.saved().unwrap().contains("skipped=0.0.3"));
    let mut s = services("0.0.2", true, Integration::Unavailable);
    s.now = t0 + 7 * DAY;
    s.check_result = Some(available("0.0.3", UpdateHow::Install));
    r.restart(s);
    assert_eq!(r.svc().calls, ["check"]);
    assert!(r.title().is_none(), "a skipped version is not announced again");
    // A newer one is.
    let mut s = services("0.0.2", true, Integration::Unavailable);
    s.now = t0 + 9 * DAY;
    s.check_result = Some(available("0.0.4", UpdateHow::Install));
    r.restart(s);
    assert_eq!(r.title().as_deref(), Some("Rusty Wave 0.0.4 is available"));
    // "Later" closes it and leaves the offer for the menu.
    r.press("Later");
    assert!(r.title().is_none());
    r.do_action(Action::CheckForUpdates);
    assert_eq!(r.title().as_deref(), Some("Rusty Wave 0.0.4 is available"));
}

#[test]
fn a_clock_set_back_does_not_silence_the_check_forever() {
    let mut r = Rig::with(Some(services("0.0.2", true, Integration::Unavailable)));
    r.host.storage.0.insert(
        APP_SETTINGS_KEY.into(),
        format!("rvp-app-settings 1\nauto_check=1\nlast_check={}\n", 1_800_000_000 + 400 * DAY).into_bytes(),
    );
    r.restart(services("0.0.2", true, Integration::Unavailable));
    assert_eq!(r.svc().calls, ["check"]);
}

#[test]
fn declining_the_app_menu_offer_is_final_by_button_or_by_closing() {
    let mut r = Rig::with(Some(services("0.0.2", false, Integration::Off)));
    assert_eq!(r.title().as_deref(), Some("Add Rusty Wave to the app menu?"));
    assert_eq!(r.buttons(), ["Add to the app menu", "No thanks"]);
    // Closing it (the X, Escape) without answering is a no too: it is never asked again.
    r.do_action(Action::DialogClose);
    assert!(r.title().is_none());
    assert_eq!(r.settings().integration, IntegrationChoice::Never);
    r.restart(services("0.0.2", false, Integration::Off));
    assert!(r.title().is_none(), "not asked again");
    // The button does the same, and is saved.
    let mut r = Rig::with(Some(services("0.0.2", false, Integration::Off)));
    r.press("No thanks");
    assert_eq!(r.settings().integration, IntegrationChoice::Never);
    assert!(r.saved().unwrap().contains("integration=never"));
    r.restart(services("0.0.2", false, Integration::Off));
    assert!(r.title().is_none());
    assert!(r.svc().calls.is_empty());
    // The Settings dialog offers it whenever the user wants (and the same for removing it).
    r.do_action(Action::ShowSettings);
    assert_eq!(r.title().as_deref(), Some("Settings"));
    assert!(r.buttons().contains(&"Add to app menu".to_string()), "{:?}", r.buttons());
    r.press("Add to app menu");
    assert_eq!(r.svc().calls, ["integrate:on"]);
    assert!(r.buttons().contains(&"Remove from app menu".to_string()), "{:?}", r.buttons());
    r.press("Remove from app menu");
    r.svc().calls.clear();
    r.press("Close");
    assert!(r.title().is_none());
    // The menu entry still works, and removing it again keeps it from being offered.
    r.do_action(Action::ToggleIntegration);
    assert_eq!(r.svc().calls, ["integrate:on"]);
    assert_eq!(r.settings().integration, IntegrationChoice::Done);
    r.do_action(Action::ToggleIntegration);
    assert_eq!(r.svc().calls, ["integrate:on", "integrate:off"]);
    assert_eq!(r.settings().integration, IntegrationChoice::Never);
}

#[test]
fn adding_from_the_offer_works_and_a_failure_is_shown_in_the_dialog() {
    let mut r = Rig::with(Some(services("0.0.2", false, Integration::Off)));
    r.svc().integration_error = Some("the folder is read-only".into());
    r.press("Add to the app menu");
    assert!(r.title().is_some());
    assert!(r.body().contains("Couldn't add it: the folder is read-only"), "{}", r.body());
    assert_eq!(r.buttons(), ["Close"]);
    r.press("Close");
    assert!(r.title().is_none());
    r.restart(services("0.0.2", false, Integration::Off));
    r.press("Add to the app menu");
    assert!(r.title().is_none());
    assert_eq!(r.svc().calls, ["integrate:on"]);
    assert_eq!(r.settings().integration, IntegrationChoice::Done);
    // Already integrated by hand before the first offer: no offer, remembered as done.
    let mut r = Rig::with(Some(services("0.0.2", false, Integration::On)));
    assert!(r.title().is_none());
    assert_eq!(r.settings().integration, IntegrationChoice::Done);
    assert!(r.svc().calls.is_empty());
}

#[test]
fn a_finished_update_asks_for_its_restart_wherever_the_user_was() {
    let mut r = Rig::with(Some(services("0.0.2", true, Integration::Unavailable)));
    r.svc().state = UpdateState::Ready { version: "0.0.3".into() };
    r.run(100);
    assert_eq!(r.title().as_deref(), Some("Restart to finish updating"));
    r.press("Later");
    r.run(300);
    assert!(r.title().is_none(), "asked once per run");
}

#[test]
fn the_dialog_is_modal_for_the_keyboard_and_the_mouse() {
    let mut r = Rig::with(Some(services("0.0.2", true, Integration::Unavailable)));
    r.do_action(Action::CheckForUpdates);
    r.svc().state = available("0.0.3", UpdateHow::Install);
    r.run(50);
    // Keys meant for the player do nothing (here: `o` would open the file picker, `f` fullscreen).
    r.key(Key::Char('f'));
    r.key(Key::Char('o'));
    assert!(!r.host.surface.fullscreen);
    assert!(r.app.take_effects().is_empty());
    assert!(r.title().is_some());
    // Enter presses the primary button.
    r.key(Key::Enter);
    assert_eq!(r.svc().calls.last().unwrap(), "install");
    // Escape closes.
    r.key(Key::Escape);
    assert!(r.title().is_none());

    // A real click on a button, found through the geometry.
    r.do_action(Action::CheckForUpdates);
    r.svc().state = available("0.0.3", UpdateHow::Install);
    r.run(50);
    let spec = r.app.model().dialog.clone().unwrap();
    let g = r.app.ui_mut().dialog_geom(&spec);
    let later = g.buttons[1];
    r.click(later.cx(), later.cy());
    assert!(r.title().is_none());
}

#[test]
fn the_right_click_menu_reaches_the_update_check() {
    let mut r = Rig::with(Some(services("0.0.2", true, Integration::Unavailable)));
    r.send(InputEvent::PointerMove { x: 600.0, y: 300.0 });
    r.send(InputEvent::PointerDown { x: 600.0, y: 300.0, button: PointerButton::Secondary });
    r.send(InputEvent::PointerUp { x: 600.0, y: 300.0, button: PointerButton::Secondary });
    let rows = r.app.ui().menu_rows();
    let (_, rect, enabled) = rows
        .iter()
        .find(|(l, ..)| l.starts_with("Check for updates"))
        .expect("the entry is in the menu")
        .clone();
    assert!(enabled);
    r.click(rect.cx(), rect.cy());
    assert_eq!(r.svc().calls, ["check"]);
    assert_eq!(r.title().as_deref(), Some("Updates"));
}

/// Writes pictures of the dialogs when `RVP_DUMP_DIALOGS=<dir>` is set (for looking at them; not an assertion).
#[test]
fn dump_pictures_of_the_dialogs_on_request() {
    let Some(dir) = std::env::var_os("RVP_DUMP_DIALOGS") else { return };
    let dir = std::path::PathBuf::from(dir);
    let mut r = Rig::with(Some(services("0.0.2", true, Integration::Off)));
    let shot = |r: &mut Rig, name: &str| {
        r.run(100);
        let fb = r.app.framebuffer();
        let (w, h) = (fb.width, fb.height);
        let mut raw = format!("P7\nWIDTH {w}\nHEIGHT {h}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n")
            .into_bytes();
        raw.extend_from_slice(&fb.pixels);
        std::fs::write(dir.join(format!("{name}.pam")), raw).unwrap();
    };
    shot(&mut r, "offer");
    r.press("Not now");
    r.do_action(Action::CheckForUpdates);
    r.svc().state = available("0.0.3", UpdateHow::Install);
    shot(&mut r, "available");
    r.press("Update now");
    r.svc().state = UpdateState::Downloading { done: 420, total: 1000 };
    shot(&mut r, "downloading");
    r.svc().state = UpdateState::Ready { version: "0.0.3".into() };
    shot(&mut r, "ready");
}

// ---- Settings and the default media player ----------------------------------------------------------------------------------------

fn with_default(silent: bool) -> ScriptedServices {
    ScriptedServices {
        default_player: DefaultPlayer::Available {
            note: if silent { String::new() } else { "Windows asks you to confirm.".into() },
            silent,
        },
        ..services("0.0.2", true, Integration::Unavailable)
    }
}

#[test]
fn settings_lists_what_the_host_can_do_and_works_without_a_host() {
    let mut r = Rig::with(None);
    r.do_action(Action::ShowSettings);
    assert_eq!(r.title().as_deref(), Some("Settings"));
    assert_eq!(
        r.buttons(),
        [
            "Audio settings\u{2026}",
            "Theme\u{2026}",
            "Visualizer order: in turn",
            "Visualizer cycle time: 1 min",
            "Close"
        ]
    );
    r.press("Close");
    assert!(r.title().is_none());
    // With services that offer everything.
    let mut s = with_default(true);
    s.integration = Integration::Off;
    let mut r = Rig::with(Some(s));
    r.press("No thanks"); // the app-menu offer, so the dialogs do not overlap
    r.do_action(Action::ShowSettings);
    assert_eq!(
        r.buttons(),
        [
            "Audio settings\u{2026}",
            "Theme\u{2026}",
            "Visualizer order: in turn",
            "Visualizer cycle time: 1 min",
            "Set as default media player\u{2026}",
            "Add to app menu",
            "Check for updates\u{2026}",
            "Close"
        ]
    );
    // Audio settings opens its own panel.
    r.press("Audio settings\u{2026}");
    assert!(r.app.model().dialog.is_none() && r.app.ui().audio_settings_open());
}

#[test]
fn the_default_player_checklist_has_every_media_type_ticked_and_sets_the_chosen_ones() {
    let mut r = Rig::with(Some(with_default(true)));
    // The first-run offer: the checklist, with a "no thanks" that is final.
    assert_eq!(r.title().as_deref(), Some("Make Rusty Wave your default media player?"));
    assert_eq!(r.app.model().dialog.as_ref().unwrap().toggles.len(), MEDIA_TYPES.len());
    assert!(r.app.model().dialog.as_ref().unwrap().toggles.iter().all(|t| t.on), "all ticked to start with");
    assert_eq!(r.buttons(), ["Set as default", "Uncheck all", "No thanks"]);
    // Untick the first two kinds and set the rest.
    r.do_action(Action::DialogToggle(0));
    r.do_action(Action::DialogToggle(1));
    r.press("Set as default");
    let want: Vec<&str> = MEDIA_TYPES.iter().skip(2).map(|t| t.id).collect();
    assert_eq!(r.svc().calls, [format!("default:{}", want.join(","))]);
    assert!(r.title().is_none());
    assert_eq!(r.settings().default_player, IntegrationChoice::Done);
    assert!(r.saved().unwrap().contains("default_player=done"));
    // Done is not offered again; Settings still has the button, and the checklist starts with everything ticked.
    r.restart(with_default(true));
    assert!(r.title().is_none());
    r.do_action(Action::ShowSettings);
    r.press("Set as default media player\u{2026}");
    assert_eq!(r.title().as_deref(), Some("Set as default media player"));
    assert!(r.app.model().dialog.as_ref().unwrap().toggles.iter().all(|t| t.on));
    // "Uncheck all" leaves nothing to set: the main button is greyed.
    r.press("Uncheck all");
    assert!(!r.app.model().dialog.as_ref().unwrap().buttons[0].enabled);
    assert_eq!(r.buttons()[1], "Check all");
    // The Back control (and Escape) return to Settings.
    assert_eq!(r.app.model().dialog.as_ref().unwrap().back.as_deref(), Some("Settings"));
    r.do_action(Action::DialogBack);
    assert_eq!(r.title().as_deref(), Some("Settings"));
}

/// Every page of Settings has a Back control, Escape and Backspace go up one level (Backspace not where a text box takes it), the X closes
/// the whole thing, and on the way back the keyboard is on the button that opened the page.
#[test]
fn every_page_of_settings_goes_back_with_the_keyboard_on_the_button_that_opened_it() {
    let mut r = Rig::with(Some(with_default(false)));
    let focused = |r: &Rig| -> String {
        let d = r.app.model().dialog.clone().unwrap();
        let i = r.app.ui().dialog_focus(&d).map_or(usize::MAX, |c| match c {
            rvp_ui::dialog::DialogControl::Button(n) => n as usize,
            _ => usize::MAX,
        });
        d.buttons.get(i).map(|b| b.label.clone()).unwrap_or_default()
    };
    // Settings itself has no Back control.
    r.do_action(Action::ShowSettings);
    assert_eq!(r.title().as_deref(), Some("Settings"));
    assert!(r.app.model().dialog.as_ref().unwrap().back.is_none());
    for (page, title) in [
        ("Theme\u{2026}", "Theme"),
        ("Set as default media player\u{2026}", "Set as default media player"),
        ("Check for updates\u{2026}", "Updates"),
    ] {
        // By the button...
        r.press(page);
        assert_eq!(r.title().as_deref(), Some(title), "{page}");
        assert_eq!(
            r.app.model().dialog.as_ref().unwrap().back.as_deref(),
            Some("Settings"),
            "{title} has a Back control"
        );
        r.do_action(Action::DialogBack);
        assert_eq!(r.title().as_deref(), Some("Settings"), "{title}: back");
        assert_eq!(focused(&r), page, "{title}: the keyboard is on the button that opened it");
        // ...by Escape...
        r.press(page);
        r.key(Key::Escape);
        assert_eq!(r.title().as_deref(), Some("Settings"), "{title}: Escape");
        assert_eq!(focused(&r), page);
        // ...by Backspace (the Theme page has a text box that takes it)...
        r.press(page);
        r.key(Key::Other("Backspace".into()));
        assert_eq!(
            r.title().as_deref(),
            Some(if title == "Theme" { "Theme" } else { "Settings" }),
            "{title}: Backspace"
        );
        if title == "Theme" {
            r.key(Key::Escape);
        }
        assert_eq!(r.title().as_deref(), Some("Settings"));
        // ...and the X closes everything.
        r.press(page);
        r.do_action(Action::DialogClose);
        assert!(r.title().is_none(), "{title}: the X closes the whole dialog");
        r.do_action(Action::ShowSettings);
    }
    // The audio page is a panel: its Back control (and Escape) return to Settings with the keyboard on its button, while the same panel
    // opened from the menu has no Back control and Escape closes it.
    r.press("Audio settings\u{2026}");
    assert!(r.app.ui().audio_settings_open() && r.title().is_none());
    r.key(Key::Escape);
    assert!(!r.app.ui().audio_settings_open());
    assert_eq!(r.title().as_deref(), Some("Settings"));
    assert_eq!(focused(&r), "Audio settings\u{2026}");
    r.do_action(Action::DialogClose);
    r.do_action(Action::ShowAudioSettings);
    assert!(r.app.ui().audio_settings_open());
    r.key(Key::Escape);
    assert!(
        !r.app.ui().audio_settings_open() && r.title().is_none(),
        "the panel opened from the menu just closes"
    );
}

#[test]
fn declining_the_default_player_offer_is_final_and_windows_says_how_it_works() {
    let mut r = Rig::with(Some(with_default(true)));
    r.do_action(Action::DialogClose);
    assert_eq!(r.settings().default_player, IntegrationChoice::Never);
    r.restart(with_default(true));
    assert!(r.title().is_none(), "not asked again");
    // Windows: the app registers itself and sends the user to Settings; the dialog says what to do there.
    let mut s = with_default(false);
    s.default_result = Some(Ok(DefaultOutcome::UserMustConfirm(
        "Windows opened Default apps. Choose Rusty Wave for each type there.".into(),
    )));
    let mut r = Rig::with(Some(s));
    assert!(r.body().contains("Windows asks you to confirm."), "{}", r.body());
    assert_eq!(r.buttons()[0], "Continue");
    r.press("Continue");
    assert!(r.body().contains("Choose Rusty Wave for each type there."), "{}", r.body());
    assert_eq!(r.buttons(), ["Close"]);
    // A failure is shown and nothing is remembered as done.
    let mut s = with_default(true);
    s.default_result = Some(Err("xdg-mime is missing".into()));
    let mut r = Rig::with(Some(s));
    r.press("Set as default");
    assert!(r.body().contains("Couldn't set it: xdg-mime is missing"), "{}", r.body());
    assert_eq!(r.settings().default_player, IntegrationChoice::Ask);
}

#[test]
fn hosts_that_cannot_set_a_default_or_switch_the_offers_off_stay_quiet() {
    // Not available (a browser, Rusty Bucket): no offer, no Settings button.
    let mut r = Rig::with(Some(services("0.0.2", true, Integration::Unavailable)));
    assert!(r.title().is_none());
    r.do_action(Action::ShowSettings);
    assert!(!r.buttons().iter().any(|b| b.contains("default")), "{:?}", r.buttons());
    // Offers switched off (tests, kiosks): the Settings button is there, the offer is not.
    let mut s = with_default(true);
    s.offers_disabled = true;
    let mut r = Rig::with(Some(s));
    assert!(r.title().is_none());
    r.do_action(Action::ShowSettings);
    assert!(r.buttons().iter().any(|b| b.contains("default")));
}
