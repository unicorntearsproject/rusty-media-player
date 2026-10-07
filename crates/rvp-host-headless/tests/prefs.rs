//! What the person sets in Settings and keeps: the tooltips switch and the visualizer's cycle (on or off, order, time). It must work on
//! a host without the update services (a browser), apply live, survive a restart, and be honest about reduced motion.
use rvp_app::{APP_SETTINGS_KEY, App, AppSettings, VIZ_CYCLE_STEPS};
use rvp_host::{HostClock, InputEvent, Key, Modifiers};
use rvp_host_headless::{DefaultCodecs, UiHost};
use rvp_ui::actions::Action;
use rvp_ui::{LibAction, Mode, UiConfig, View};
use rvp_viz::{EFFECTS, Effect};
use std::rc::Rc;

fn new_app(reduce_motion: bool) -> App {
    App::new(Rc::new(DefaultCodecs::default()), UiConfig { reduce_motion })
}

struct Rig {
    host: UiHost,
    app: App,
}

impl Rig {
    fn new(reduce_motion: bool) -> Rig {
        let mut r = Rig { host: UiHost::new(), app: new_app(reduce_motion) };
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

    fn restart(&mut self, reduce_motion: bool) {
        self.app = new_app(reduce_motion);
        self.run(100);
    }

    fn act(&mut self, a: Action) {
        let now = self.host.virtual_clock().now_us();
        self.app.apply(&mut self.host, a, now);
        self.run(50);
    }

    fn on_viz(&mut self) {
        self.app.ui_mut().set_mode(Mode::Library);
        self.app.ui_mut().show_view(View::Visualizer);
        self.run(100);
    }

    fn effect(&self) -> Effect {
        self.app.viz().effect
    }

    fn buttons(&self) -> Vec<String> {
        self.app
            .model()
            .dialog
            .as_ref()
            .map(|d| d.buttons.iter().map(|b| b.label.clone()).collect())
            .unwrap_or_default()
    }

    fn toggles(&self) -> Vec<(String, bool)> {
        self.app
            .model()
            .dialog
            .as_ref()
            .map(|d| d.toggles.iter().map(|t| (t.label.clone(), t.on)).collect())
            .unwrap_or_default()
    }

    fn press(&mut self, label: &str) {
        let i = self
            .buttons()
            .iter()
            .position(|b| b.starts_with(label))
            .unwrap_or_else(|| panic!("no {label}: {:?}", self.buttons()));
        self.act(Action::DialogButton(i as u8));
    }

    fn flip(&mut self, label: &str) {
        let i = self
            .toggles()
            .iter()
            .position(|t| t.0.starts_with(label))
            .unwrap_or_else(|| panic!("no {label}: {:?}", self.toggles()));
        self.act(Action::DialogToggle(i as u8));
    }

    fn saved(&self) -> AppSettings {
        let text = String::from_utf8(self.host.storage.0.get(APP_SETTINGS_KEY).cloned().unwrap_or_default())
            .unwrap();
        AppSettings::from_text(&text).unwrap_or_default()
    }
}

#[test]
fn the_settings_text_round_trips_and_refuses_what_is_not_offered() {
    let s = AppSettings {
        tooltips: false,
        viz_cycle: true,
        viz_random: true,
        viz_secs: 300,
        ..AppSettings::default()
    };
    assert_eq!(AppSettings::from_text(&s.to_text()), Some(s));
    // Older files (no such lines) mean the defaults: tooltips on, cycle off, 1 min.
    let old = AppSettings::from_text("rvp-app-settings 1\nauto_check=1\n").unwrap();
    assert!(old.tooltips && !old.viz_cycle && !old.viz_random && old.viz_secs == 60 && old.auto_check);
    // A hand-edited time that is not one of the steps (0, a flicker, a day) falls back to the default.
    for bad in ["0", "1", "5", "86400", "x", ""] {
        let t = AppSettings::from_text(&format!("rvp-app-settings 1\nviz_secs={bad}\n")).unwrap();
        assert_eq!(t.viz_secs, 60, "{bad:?}");
    }
    for step in VIZ_CYCLE_STEPS {
        let t = AppSettings::from_text(&format!("rvp-app-settings 1\nviz_secs={step}\n")).unwrap();
        assert_eq!(t.viz_secs, step);
    }
    assert_eq!((VIZ_CYCLE_STEPS[0], *VIZ_CYCLE_STEPS.last().unwrap()), (15, 600));
}

#[test]
fn settings_offers_the_tooltip_switch_and_the_cycle_controls_without_any_host_services() {
    let mut r = Rig::new(false);
    assert!(r.host.services.is_none(), "a host like a browser's");
    r.act(Action::ShowSettings);
    let labels = r.buttons();
    assert!(labels.iter().any(|b| b == "Visualizer order: in turn"), "{labels:?}");
    assert!(labels.iter().any(|b| b == "Visualizer cycle time: 1 min"), "{labels:?}");
    assert_eq!(
        r.toggles().iter().map(|t| t.1).collect::<Vec<_>>(),
        [true, false, false],
        "tooltips on, cycle off, history on"
    );
    // Each press is applied at once, and kept.
    r.flip("Show tooltips");
    assert!(!r.app.app_settings().tooltips && !r.saved().tooltips);
    r.flip("Change the visualizer");
    assert!(r.app.app_settings().viz_cycle && r.saved().viz_cycle);
    r.press("Visualizer order");
    assert!(r.saved().viz_random);
    assert!(r.buttons().iter().any(|b| b == "Visualizer order: random"));
    // The time steps through 15 s ... 10 min and round again.
    let mut seen = Vec::new();
    for _ in 0..VIZ_CYCLE_STEPS.len() {
        r.press("Visualizer cycle time");
        seen.push(r.saved().viz_secs);
    }
    assert_eq!(seen, [120, 300, 600, 15, 30, 60]);
    // A restart finds it all as it was left.
    r.restart(false);
    let s = r.app.app_settings().clone();
    assert!(!s.tooltips && s.viz_cycle && s.viz_random && s.viz_secs == 60);
}

#[test]
fn the_cycle_steps_through_the_effects_in_turn_after_the_set_time() {
    let mut r = Rig::new(false);
    r.on_viz();
    r.act(Action::ShowSettings);
    for _ in 0..4 {
        r.press("Visualizer cycle time"); // 1 min -> 2 -> 5 -> 10 min -> 15 s
    }
    assert_eq!(r.app.app_settings().viz_secs, 15);
    r.act(Action::DialogClose);
    r.on_viz();
    let start = r.effect();
    // Off: nothing changes however long it runs.
    r.run(20_000);
    assert_eq!(r.effect(), start);
    // Shift+V on the visualizer switches it on.
    r.app.ui_mut().show_view(View::Visualizer);
    let now = r.host.virtual_clock().now_us();
    r.app.apply(&mut r.host, Action::Lib(LibAction::VizCycle), now);
    assert!(r.app.app_settings().viz_cycle);
    r.run(14_000);
    assert_eq!(r.effect(), start, "not before 15 s");
    r.run(3_000);
    assert_eq!(r.effect(), start.step(1));
    r.run(15_500);
    assert_eq!(r.effect(), start.step(2));
    // Choosing an effect by hand starts the time again.
    let now = r.host.virtual_clock().now_us();
    r.app.apply(&mut r.host, Action::Lib(LibAction::VizStep(1)), now);
    let by_hand = r.effect();
    r.run(10_000);
    assert_eq!(r.effect(), by_hand);
    r.run(6_000);
    assert_eq!(r.effect(), by_hand.step(1));
    // Off again.
    let now = r.host.virtual_clock().now_us();
    r.app.apply(&mut r.host, Action::Lib(LibAction::VizCycle), now);
    let e = r.effect();
    r.run(40_000);
    assert_eq!(r.effect(), e);
}

#[test]
fn random_order_never_repeats_the_showing_effect_and_uses_every_effect() {
    let mut r = Rig::new(false);
    r.act(Action::ShowSettings);
    r.press("Visualizer order");
    for _ in 0..4 {
        r.press("Visualizer cycle time"); // 1 min -> 2 -> 5 -> 10 min -> 15 s
    }
    assert_eq!(r.app.app_settings().viz_secs, 15);
    r.act(Action::DialogClose);
    r.on_viz();
    let now = r.host.virtual_clock().now_us();
    r.app.apply(&mut r.host, Action::Lib(LibAction::VizCycle), now);
    let mut seen = std::collections::BTreeSet::new();
    let mut last = r.effect();
    seen.insert(format!("{last:?}"));
    for _ in 0..60 {
        r.run(15_100);
        let e = r.effect();
        assert_ne!(e, last, "the same effect twice in a row");
        seen.insert(format!("{e:?}"));
        last = e;
    }
    assert_eq!(seen.len(), EFFECTS.len(), "every effect turns up: {seen:?}");
}

#[test]
fn reduced_motion_keeps_the_effect_still_and_says_so() {
    let mut r = Rig::new(true);
    r.on_viz();
    let start = r.effect();
    let now = r.host.virtual_clock().now_us();
    r.app.apply(&mut r.host, Action::Lib(LibAction::VizCycle), now);
    assert!(r.app.app_settings().viz_cycle, "the choice is kept");
    r.run(70_000);
    assert_eq!(r.effect(), start);
    // Shift+V reaches it from the keyboard on the visualizer.
    r.app.ui_mut().show_view(View::Visualizer);
    let before = r.app.app_settings().viz_cycle;
    r.host.input.0.push_back(InputEvent::KeyDown {
        key: Key::Char('V'),
        mods: Modifiers { shift: true, ..Modifiers::default() },
        repeat: false,
    });
    r.app.pump(&mut r.host);
    r.run(50);
    assert_ne!(r.app.app_settings().viz_cycle, before);
}
