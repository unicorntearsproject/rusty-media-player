//! The first run and the last face: the app opens on the Library the first time, adds the system's Music and Videos folders when the
//! library is empty, and later runs open on the face the last one ended on (a Player face with nothing to show falls back to the Library).
use rvp_app::{App, SETUP_KEY};
use rvp_host::{HostClock, ScriptedLibrary, StandardFolder, StandardKind};
use rvp_host_headless::{DefaultCodecs, UiHost};
use rvp_ui::actions::Action;
use rvp_ui::{Mode, UiConfig};
use std::rc::Rc;

fn new_app() -> App {
    App::new(Rc::new(DefaultCodecs { stall: None, clock: None }), UiConfig { reduce_motion: true })
}

fn standard() -> Vec<StandardFolder> {
    vec![
        StandardFolder { kind: StandardKind::Music, name: "Music".into(), path: "/home/u/Music".into() },
        StandardFolder { kind: StandardKind::Videos, name: "Videos".into(), path: "/home/u/Videos".into() },
    ]
}

struct Rig {
    host: UiHost,
    app: App,
}

impl Rig {
    fn new() -> Rig {
        let mut host = UiHost::new();
        host.library = Some(ScriptedLibrary { standard: standard(), ..ScriptedLibrary::default() });
        let mut r = Rig { host, app: new_app() };
        r.run(300);
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

    /// The next run of the program: the same storage, a new app (and a library host that has not been asked anything yet).
    fn restart(&mut self) {
        self.app = new_app();
        self.host.library = Some(ScriptedLibrary { standard: standard(), ..ScriptedLibrary::default() });
        self.run(300);
    }

    fn added(&self) -> Vec<String> {
        // `UiHost` keeps the scripted library; the paths the app asked it to add.
        self.host.library.as_ref().map(|l| l.added.clone()).unwrap_or_default()
    }

    fn saved(&self) -> String {
        self.host.storage.0.get(SETUP_KEY).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default()
    }
}

#[test]
fn the_first_run_opens_on_the_library_and_adds_the_standard_folders() {
    let r = Rig::new();
    assert_eq!(r.app.ui().mode(), Mode::Library);
    assert!(r.app.setup().is_first_run());
    assert_eq!(r.added(), ["/home/u/Music", "/home/u/Videos"]);
    assert!(r.saved().contains("face=library") && r.saved().contains("folders=done"), "{}", r.saved());
}

#[test]
fn the_standard_folders_are_added_once_and_not_to_a_library_that_has_folders() {
    let mut r = Rig::new();
    // The next run does not add them again (the user may have taken them out on purpose).
    r.restart();
    assert!(!r.app.setup().is_first_run());
    assert!(r.added().is_empty(), "{:?}", r.added());
}

#[test]
fn a_host_without_standard_folders_just_opens_on_the_library() {
    let mut host = UiHost::new();
    host.library = Some(ScriptedLibrary::default());
    let mut app = new_app();
    let clock = host.virtual_clock();
    for _ in 0..30 {
        app.tick(&mut host);
        clock.advance(16_000);
    }
    assert_eq!(app.ui().mode(), Mode::Library);
    assert!(host.library.as_ref().unwrap().added.is_empty());
    // And one with no library capability at all (a browser without directory access) too.
    let mut host = UiHost::new();
    let mut app = new_app();
    for _ in 0..30 {
        app.tick(&mut host);
        clock.advance(16_000);
    }
    assert_eq!(app.ui().mode(), Mode::Library);
}

#[test]
fn later_runs_open_on_the_last_face_and_an_empty_player_falls_back_to_the_library() {
    let mut r = Rig::new();
    // Leave on the Player face with nothing playing: the next run has nothing to show there, so it lands on the Library.
    let now = r.host.virtual_clock().now_us();
    r.app.apply(&mut r.host, Action::SetMode(Mode::Player), now);
    r.run(100);
    assert!(r.saved().contains("face=player"), "{}", r.saved());
    r.restart();
    assert_eq!(r.app.ui().mode(), Mode::Library);
    assert!(r.saved().contains("face=library"), "{}", r.saved());
    // The Library face is kept as it is.
    r.restart();
    assert_eq!(r.app.ui().mode(), Mode::Library);
}
