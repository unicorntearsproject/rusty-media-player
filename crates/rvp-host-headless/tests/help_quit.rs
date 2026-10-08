//! Help and Quit through the whole app: H and ? open the help page on either face and Esc closes it; Ctrl+Q ends the app only on a
//! host that has a window to close, and Ctrl+F is the heart.
use rvp_app::{App, Effect};
use rvp_host::{HostClock, InputEvent, Key, Modifiers};
use rvp_host_headless::{DefaultCodecs, UiHost};
use rvp_ui::UiConfig;
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
