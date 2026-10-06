//! The Theme dialog through the whole app: paste CSS (or a link the scripted network answers), preview the look live, apply it (kept per
//! user), close without applying (the old look comes back), and what is said when a link cannot be read. The colours are process-wide, so
//! the tests take turns.
use rvp_app::App;
use rvp_host::{HostClock, InputEvent, ScriptedNet};
use rvp_host_headless::{DefaultCodecs, UiHost};
use rvp_ui::actions::Action;
use rvp_ui::theming::THEME_KEY;
use rvp_ui::{UiConfig, tk};
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard};

fn one_at_a_time() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const CSS: &str =
    ":root{--background:#102030;--card:#1a2c40;--foreground:#f2f6ff;--primary:#00c2a8;--radius:4px}";

struct Rig {
    host: UiHost,
    app: App,
}

fn new_app() -> App {
    App::new(Rc::new(DefaultCodecs { stall: None, clock: None }), UiConfig { reduce_motion: true })
}

impl Rig {
    fn new() -> Rig {
        tk::reset();
        rvp_ui::gfx::set_radius_scale(1.0);
        let mut r = Rig { host: UiHost::new(), app: new_app() };
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

    fn act(&mut self, a: Action) {
        let now = self.host.virtual_clock().now_us();
        self.app.apply(&mut self.host, a, now);
        self.run(50);
    }

    fn title(&self) -> Option<String> {
        self.app.model().dialog.as_ref().map(|d| d.title.clone())
    }

    fn body(&self) -> String {
        self.app.model().dialog.as_ref().map(|d| d.body.join(" | ")).unwrap_or_default()
    }

    fn press(&mut self, label: &str) {
        let labels: Vec<String> = self
            .app
            .model()
            .dialog
            .as_ref()
            .map(|d| d.buttons.iter().map(|b| b.label.clone()).collect())
            .unwrap_or_default();
        let i =
            labels.iter().position(|b| b == label).unwrap_or_else(|| panic!("no button {label}: {labels:?}"));
        self.act(Action::DialogButton(i as u8));
    }

    fn open_theme(&mut self) {
        self.act(Action::ShowSettings);
        self.press("Theme\u{2026}");
        assert_eq!(self.title().as_deref(), Some("Theme"));
    }

    fn paste(&mut self, text: &str) {
        self.host.input.0.push_back(InputEvent::Paste(text.to_string()));
        self.app.pump(&mut self.host);
        self.run(50);
    }

    fn input_text(&self) -> String {
        self.app
            .model()
            .dialog
            .as_ref()
            .and_then(|d| d.input.as_ref())
            .map(|i| i.text.clone())
            .unwrap_or_default()
    }

    fn saved(&self) -> Option<String> {
        self.host
            .storage
            .0
            .get(THEME_KEY)
            .filter(|b| !b.is_empty())
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }
}

#[test]
fn pasted_css_previews_live_and_apply_keeps_it_for_the_next_run() {
    let _t = one_at_a_time();
    let mut r = Rig::new();
    let before = tk::get();
    r.open_theme();
    r.paste(CSS);
    assert_eq!(r.input_text(), CSS, "a short paste shows as it is; a long one is summarised");
    r.press("Preview");
    // The preview is on screen at once, and the dialog says what it found.
    assert_eq!(tk::ink_900(), rvp_ui::theming::color::parse_color("#102030").unwrap());
    assert_ne!(tk::get(), before);
    assert!(r.body().contains("Previewing Pasted theme"), "{}", r.body());
    assert!(r.saved().is_none(), "a preview is not kept yet");
    r.press("Apply");
    assert!(r.saved().unwrap().starts_with("rvp-theme 1\nname=Pasted theme\n"), "{:?}", r.saved());
    assert_eq!(r.app.theme_name(), "Pasted theme");
    // Close, and the next run starts with it.
    r.press("Close");
    tk::reset();
    let host = std::mem::replace(&mut r.host, UiHost::new());
    let mut again = Rig { host, app: new_app() };
    again.run(100);
    assert_eq!(tk::ink_900(), rvp_ui::theming::color::parse_color("#102030").unwrap());
    assert_eq!(again.app.theme_name(), "Pasted theme");
    // Reset puts Unicorn Tears back and forgets the saved theme.
    again.open_theme();
    again.press("Reset to Unicorn Tears");
    assert_eq!(tk::get(), rvp_ui::tk::Colors::DEFAULT);
    assert!(again.saved().is_none());
    tk::reset();
}

#[test]
fn closing_without_applying_brings_the_old_look_back() {
    let _t = one_at_a_time();
    let mut r = Rig::new();
    r.open_theme();
    r.paste(CSS);
    r.press("Preview");
    assert_ne!(tk::get(), rvp_ui::tk::Colors::DEFAULT);
    r.act(Action::DialogClose);
    assert_eq!(tk::get(), rvp_ui::tk::Colors::DEFAULT, "an unapplied preview goes away");
    assert!(r.saved().is_none());
    // Nothing to apply before a preview: the button is greyed.
    r.act(Action::ShowSettings);
    r.press("Theme\u{2026}");
    let d = r.app.model().dialog.clone().unwrap();
    assert!(!d.buttons.iter().find(|b| b.label == "Apply").unwrap().enabled);
    tk::reset();
}

#[test]
fn typing_edits_the_box_and_what_is_not_css_is_said_in_words() {
    let _t = one_at_a_time();
    let mut r = Rig::new();
    r.open_theme();
    for c in "hello".chars() {
        r.act(Action::DialogChar(c));
    }
    assert_eq!(r.input_text(), "hello");
    r.act(Action::DialogBackspace);
    assert_eq!(r.input_text(), "hell");
    r.press("Preview");
    assert!(r.body().contains("no design tokens"), "{}", r.body());
    assert_eq!(tk::get(), rvp_ui::tk::Colors::DEFAULT);
    // An empty box asks for something.
    for _ in 0..4 {
        r.act(Action::DialogBackspace);
    }
    r.press("Preview");
    assert!(r.body().contains("Paste a link or some CSS first"), "{}", r.body());
    // CSS with tokens but no colours.
    r.paste(":root{--space-4:1rem}");
    r.press("Preview");
    assert!(r.body().contains("no page, text or accent colors"), "{}", r.body());
    tk::reset();
}

#[test]
fn a_link_is_fetched_with_its_stylesheets_and_imports_and_read_in_order() {
    let _t = one_at_a_time();
    let mut r = Rig::new();
    let mut net = ScriptedNet::default();
    net.delay_polls = 3;
    net.pages.insert(
        "https://design.example/system/index.html".into(),
        Ok("<html><head><link rel=\"stylesheet\" href=\"styles.css\"><style>:root{--radius:2px}</style></head></html>".into()),
    );
    net.pages.insert(
        "https://design.example/system/styles.css".into(),
        Ok("@import url('tokens/colors.css'); :root{--primary:#ff8a00}".into()),
    );
    net.pages.insert(
        "https://design.example/system/tokens/colors.css".into(),
        Ok(":root{--background:#0b1220;--foreground:#eef2ff;--primary:#2255ff}".into()),
    );
    r.host.net = Some(net);
    r.open_theme();
    r.paste("https://design.example/system/index.html");
    assert_eq!(r.input_text(), "https://design.example/system/index.html");
    r.press("Preview");
    assert!(r.body().contains("Fetching"), "{}", r.body());
    r.run(500);
    // All three were asked for, the page first.
    assert_eq!(
        r.host.net.as_ref().unwrap().requests,
        [
            "https://design.example/system/index.html",
            "https://design.example/system/styles.css",
            "https://design.example/system/tokens/colors.css"
        ]
    );
    assert!(r.body().contains("Previewing design.example"), "{}", r.body());
    // The importing sheet's own primary wins over the one it imported; the imported file supplies the page and the text.
    assert_eq!(tk::ink_900(), rvp_ui::theming::color::parse_color("#0b1220").unwrap());
    assert_eq!(tk::magenta_500(), rvp_ui::theming::color::parse_color("#ff8a00").unwrap());
    r.press("Apply");
    assert_eq!(r.app.theme_name(), "design.example");
    tk::reset();
}

#[test]
fn a_link_that_cannot_be_read_says_to_paste_the_css_and_changes_nothing() {
    let _t = one_at_a_time();
    let mut r = Rig::new();
    let mut net = ScriptedNet::default();
    net.pages.insert("https://private.example/x".into(), Err("the server said 401".into()));
    r.host.net = Some(net);
    r.open_theme();
    r.paste("https://private.example/x");
    r.press("Preview");
    r.run(100);
    assert!(r.body().contains("Couldn't read that link (the server said 401)"), "{}", r.body());
    assert!(r.body().contains("paste it here instead"), "{}", r.body());
    assert_eq!(tk::get(), rvp_ui::tk::Colors::DEFAULT);
    // A host with no network at all says so too.
    let mut r2 = Rig::new();
    r2.open_theme();
    r2.paste("https://x.example/y");
    r2.press("Preview");
    assert!(r2.body().contains("cannot fetch links"), "{}", r2.body());
    tk::reset();
}
