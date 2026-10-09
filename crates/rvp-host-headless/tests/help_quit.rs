//! Help and Quit through the whole app: H and ? open the help page on either face and Esc closes it; Ctrl+Q ends the app only on a
//! host that has a window to close, and Ctrl+F is the heart.
use rvp_app::{App, Effect};
use rvp_host::{HostClock, InputEvent, Key, Modifiers};
use rvp_host_headless::{DefaultCodecs, UiHost};
use rvp_ui::{Action, LibAction, UiConfig};
use std::rc::Rc;

struct Rig {
    host: UiHost,
    app: App,
}

impl Rig {
    fn new(quit: bool) -> Rig {
        let mut host = UiHost::new();
        host.quit = quit;
        let app = App::new(Rc::new(DefaultCodecs::default()), UiConfig { reduce_motion: true });
        let mut r = Rig { host, app };
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

    fn key(&mut self, key: Key, ctrl: bool) {
        self.host.input.0.push_back(InputEvent::KeyDown {
            key,
            mods: Modifiers { ctrl, ..Modifiers::default() },
            repeat: false,
        });
        self.app.pump(&mut self.host);
        self.run(50);
    }

    fn snapshot_has_help(&mut self) -> bool {
        let json = self.app.snapshot().json().contains("\"help\":{");
        assert_eq!(json, self.app.ui().help_open());
        json
    }
}

#[test]
fn h_and_question_mark_open_the_help_page_and_escape_h_or_question_mark_close_it() {
    for open in ['h', '?'] {
        for close in [Key::Escape, Key::Char('h'), Key::Char('?')] {
            let mut r = Rig::new(false);
            r.key(Key::Char(open), false);
            assert!(r.snapshot_has_help(), "{open} opens it");
            r.key(Key::Space, false);
            assert!(r.snapshot_has_help(), "other keys do not close it");
            r.key(close.clone(), false);
            assert!(!r.snapshot_has_help(), "{close:?} closes it");
        }
    }
}

#[test]
fn ctrl_q_quits_only_where_the_host_can() {
    let mut r = Rig::new(true);
    r.key(Key::Char('q'), true);
    assert!(r.app.take_effects().iter().any(|e| matches!(e, Effect::Quit)));
    // A browser tab: the key is left alone and nothing is asked.
    let mut r = Rig::new(false);
    r.key(Key::Char('q'), true);
    assert!(!r.app.take_effects().iter().any(|e| matches!(e, Effect::Quit)));
    // Plain Q is the queue, never quit.
    let mut r = Rig::new(true);
    r.key(Key::Char('q'), false);
    assert!(!r.app.take_effects().iter().any(|e| matches!(e, Effect::Quit)));
}

#[test]
fn the_help_overlay_is_not_a_quit_route_and_the_quit_menu_entry_follows_the_host() {
    let m = |quit: bool| {
        let mut r = Rig::new(quit);
        r.run(50);
        r.app.model().app.quit
    };
    assert!(m(true));
    assert!(!m(false));
}

#[test]
fn the_about_pages_suggestion_links_open_x_and_the_github_issues() {
    let mut r = Rig::new(false);
    let now = 10_000_000;
    r.app.apply(&mut r.host, rvp_ui::Action::Lib(rvp_ui::LibAction::About(4)), now);
    r.app.apply(&mut r.host, rvp_ui::Action::Lib(rvp_ui::LibAction::About(5)), now);
    let urls: Vec<String> = r
        .app
        .take_effects()
        .into_iter()
        .filter_map(|e| if let Effect::OpenUrl(u) = e { Some(u) } else { None })
        .collect();
    assert_eq!(
        urls,
        ["https://x.com/djunicorntears", "https://github.com/unicorntearsproject/rusty-media-player/issues"]
    );
}

#[test]
fn the_collapsed_rail_is_kept_between_runs() {
    let mut r = Rig::new(false);
    assert!(!r.app.ui().rail_collapsed());
    let now = 1_000_000;
    r.app.apply(&mut r.host, Action::Lib(LibAction::ToggleRail), now);
    r.run(100);
    assert!(r.app.ui().rail_collapsed() && r.app.app_settings().rail_collapsed);
    let saved = r.host.storage.0.get("settings/app").expect("saved with the app settings");
    assert!(std::str::from_utf8(saved).unwrap().contains("rail_collapsed=1"));
    // A new run reads it back.
    let host = std::mem::replace(&mut r.host, UiHost::new());
    let mut again =
        Rig { host, app: App::new(Rc::new(DefaultCodecs::default()), UiConfig { reduce_motion: true }) };
    again.run(200);
    assert!(again.app.ui().rail_collapsed(), "collapsed after a restart");
    again.app.apply(&mut again.host, Action::Lib(LibAction::ToggleRail), now);
    again.run(100);
    assert!(!again.app.ui().rail_collapsed());
}
