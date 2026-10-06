//! Editing tags through the whole application: the form opens filled in from the library, a save reads each file, edits it, checks it and
//! has the host replace it; the library shows the change at once and again after the folder is read; a cover can be replaced and
//! removed; an album's shared tags change on every song; and a host that cannot write (or a write that fails) leaves every file as it was.
use rvp_app::{App, Effect};
use rvp_core::task::block_on;
use rvp_host::mock::MemSource;
use rvp_host::{FileWriter, HostClock, InputEvent, Key, Modifiers, ScriptedLibrary, ScriptedWriter};
use rvp_host_headless::{DefaultCodecs, FsWriter, UiHost, walk_listing};
use rvp_library::{TrackTags, read_tags};
use rvp_ui::{LibAction, Scope, UiConfig, UiModel};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::Once;

fn fixtures() -> PathBuf {
    static ONCE: Once = Once::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let d =
        std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root.join("target/fixtures"));
    ONCE.call_once(|| {
        if std::env::var_os("RVP_SKIP_FIXTURES").is_some() {
            return;
        }
        let st = Command::new("bash")
            .arg(root.join("tools/gen-fixtures.sh"))
            .arg(&d)
            .env("RVP_FIXTURE_SET", "audio")
            .status()
            .expect("run gen-fixtures.sh");
        assert!(st.success(), "fixture generation failed");
    });
    d.join("audio")
}

fn skip() -> bool {
    std::env::var_os("RVP_SKIP_FIXTURES").is_some()
}

const ROOT: &str = "music";

/// A music folder with two albums made of copies of the audio fixtures.
fn music_folder(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("rvp-tagedit-{tag}-{}", std::process::id()));
    let _ = std::fs::create_dir_all(d.join("Album A"));
    let _ = std::fs::create_dir_all(d.join("Album B"));
    let fx = fixtures();
    for (from, to) in [
        ("cbr.mp3", "Album A/01 One.mp3"),
        ("plain.mp3", "Album A/02 Two.mp3"),
        ("tone.flac", "Album B/03 Three.flac"),
        ("tone.opus", "Album B/04 Four.opus"),
    ] {
        std::fs::copy(fx.join(from), d.join(to)).unwrap();
    }
    d
}

fn file_tags(p: &Path) -> TrackTags {
    block_on(read_tags(MemSource::new(std::fs::read(p).unwrap()))).unwrap()
}

struct Rig {
    host: UiHost,
    app: App,
    dir: PathBuf,
}

impl Rig {
    fn new(tag: &str, writer: Option<Box<dyn FileWriter>>) -> Rig {
        let dir = music_folder(tag);
        let mut host = UiHost::new();
        host.library = Some(ScriptedLibrary::default());
        host.writer = writer;
        let mut r = Rig {
            host,
            app: App::new(Rc::new(DefaultCodecs::default()), UiConfig { reduce_motion: true }),
            dir,
        };
        r.relist();
        r.run(3000);
        r
    }

    fn with_fs(tag: &str) -> Rig {
        let dir = music_folder(tag);
        let mut w = FsWriter::default();
        w.roots.insert(ROOT.into(), dir.clone());
        let mut host = UiHost::new();
        host.library = Some(ScriptedLibrary::default());
        host.writer = Some(Box::new(w));
        let mut r = Rig {
            host,
            app: App::new(Rc::new(DefaultCodecs::default()), UiConfig { reduce_motion: true }),
            dir,
        };
        r.relist();
        r.run(3000);
        r
    }

    fn relist(&mut self) {
        let l = walk_listing(ROOT, "Music", &self.dir);
        self.host.library.as_mut().unwrap().listings.push_back(l);
    }

    fn run(&mut self, ms: i64) {
        let clock = self.host.virtual_clock();
        let end = clock.now_us() + ms * 1000;
        while clock.now_us() < end {
            self.app.tick(&mut self.host);
            // The host does what the effects ask: a folder read again is a new listing.
            for e in self.app.take_effects() {
                if let Effect::Rescan(_) = e {
                    self.relist();
                }
            }
            clock.advance(16_000);
        }
    }

    fn act(&mut self, a: rvp_ui::Action) {
        let now = self.host.virtual_clock().now_us();
        self.app.apply(&mut self.host, a, now);
        self.run(100);
    }

    fn key(&mut self, k: Key) {
        self.host.input.0.push_back(InputEvent::KeyDown {
            key: k,
            mods: Modifiers::default(),
            repeat: false,
        });
        self.app.pump(&mut self.host);
        self.run(20);
    }

    fn model(&self) -> &UiModel {
        self.app.model()
    }

    fn track_id(&self, name: &str) -> u32 {
        self.app
            .library()
            .all_tracks()
            .iter()
            .find(|t| t.path.ends_with(name))
            .unwrap_or_else(|| panic!("{name}"))
            .id
    }

    fn form(&mut self) -> serde_json::Value {
        // The editor is described by the snapshot once it has been drawn.
        self.run(40);
        let s: serde_json::Value = serde_json::from_str(self.app.snapshot().json()).unwrap();
        s["lib"]["tagform"].clone()
    }

    fn field(&mut self, key: &str) -> String {
        let f = self.form();
        f["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["key"] == key)
            .map(|x| x["value"].as_str().unwrap().to_string())
            .unwrap_or_else(|| panic!("no field {key}: {f}"))
    }

    /// Put `text` in the box of `key` by typing (Tab to it, rub out what is there).
    fn type_in(&mut self, key: &str, text: &str) {
        for _ in 0..12 {
            let f = self.form();
            let focused = f["fields"]
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["focus"] == true)
                .map(|x| x["key"].as_str().unwrap().to_string());
            if focused.as_deref() == Some(key) {
                break;
            }
            self.key(Key::Other("Tab".into()));
        }
        for _ in 0..self.field(key).chars().count() {
            self.key(Key::Other("Backspace".into()));
        }
        for c in text.chars() {
            self.key(if c == ' ' { Key::Space } else { Key::Char(c) });
        }
    }

    fn save(&mut self) {
        self.key(Key::Enter);
        self.run(8000);
    }
}

#[test]
fn a_song_is_edited_on_disk_in_the_library_and_in_search() {
    if skip() {
        return;
    }
    let mut r = Rig::with_fs("song");
    assert_eq!(r.app.library().track_count(), 4);
    assert!(r.model().app.tags, "the host can write, so the editor is offered");
    let id = r.track_id("01 One.mp3");
    r.act(rvp_ui::Action::Lib(LibAction::EditTags(Scope::Track(id))));
    assert!(r.app.ui().tag_form_open());
    let f = r.form();
    assert_eq!(f["title"], "Edit tags");
    assert_eq!(f["subtitle"], "01 One.mp3");
    assert_eq!(f["read_only"], serde_json::Value::Null);
    let before = r.app.library().track(id).unwrap().clone();
    assert_eq!(r.field("title"), before.title);
    assert_eq!(r.field("artist"), before.artist);
    r.type_in("title", "Caf\u{e9} Zeppelin");
    r.type_in("artist", "The Quokkas");
    r.type_in("year", "2011");
    r.type_in("track_no", "7");
    r.type_in("genre", "Dub");
    r.save();
    assert!(!r.app.ui().tag_form_open());
    // On disk.
    let t = file_tags(&r.dir.join("Album A/01 One.mp3"));
    assert_eq!(
        (t.title.as_str(), t.artist.as_str(), t.year, t.track_no, t.genre.as_str()),
        ("Caf\u{e9} Zeppelin", "The Quokkas", 2011, 7, "Dub")
    );
    assert_eq!(t.album, before.album, "what was not edited is as it was");
    // In the library, and found by search right away.
    let now = r.app.library().track(id).unwrap().clone();
    assert_eq!(
        (now.title.as_str(), now.artist.as_str(), now.year),
        ("Caf\u{e9} Zeppelin", "The Quokkas", 2011)
    );
    assert_eq!(r.app.library().search("quokkas").tracks, [id]);
    assert_eq!(r.app.library().search("cafe zeppelin").tracks, [id], "accents and case do not matter");
    assert!(
        r.app.library().search(&before.artist).tracks.iter().all(|&t| t != id) || before.artist.is_empty()
    );
    // No temporary file is left behind and the other songs are untouched.
    let names: Vec<String> = std::fs::read_dir(r.dir.join("Album A"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.contains("tmp")), "{names:?}");
    // Opening the editor again shows what was saved.
    r.act(rvp_ui::Action::Lib(LibAction::EditTags(Scope::Track(id))));
    assert_eq!(r.field("title"), "Caf\u{e9} Zeppelin");
    assert_eq!(r.field("track_no"), "7");
    // Cancel changes nothing and nothing is saved when nothing changed.
    r.key(Key::Enter);
    r.run(500);
    assert!(!r.app.ui().tag_form_open());
}

#[test]
fn an_albums_shared_tags_change_on_every_song_and_the_cover_can_be_replaced_and_removed() {
    if skip() {
        return;
    }
    let mut r = Rig::with_fs("album");
    // The two MP3s share an album first (their own tags differ): one song at a time through the editor.
    for name in ["01 One.mp3", "02 Two.mp3"] {
        let id = r.track_id(name);
        r.act(rvp_ui::Action::Lib(LibAction::EditTags(Scope::Track(id))));
        r.type_in("album", "Pre Album");
        r.type_in("album_artist", "Pre Artist");
        r.save();
    }
    let id1 = r.track_id("01 One.mp3");
    let album = r.app.library().album_of(id1).expect("an album").id;
    assert_eq!(r.app.library().album(album).unwrap().tracks.len(), 2, "both songs are in it now");
    r.act(rvp_ui::Action::Lib(LibAction::EditTags(Scope::Album(album))));
    let f = r.form();
    assert_eq!(f["title"], "Edit album tags");
    assert!(f["subtitle"].as_str().unwrap().starts_with("2 songs"));
    let keys: Vec<&str> =
        f["fields"].as_array().unwrap().iter().map(|x| x["key"].as_str().unwrap()).collect();
    assert!(
        !keys.contains(&"title") && !keys.contains(&"track_no"),
        "an album has no title or track number: {keys:?}"
    );
    r.type_in("album", "Quokka Hits");
    r.type_in("album_artist", "The Quokkas");
    r.type_in("year", "1999");
    // A cover: the Replace button asks the host for a picture; the host gives one.
    let jpg = std::fs::read(fixtures().join("cover.jpg")).unwrap();
    r.app.cover_picked("front.jpg", jpg.clone(), 0);
    assert_eq!(r.form()["cover"], "Replace");
    r.save();
    for n in ["Album A/01 One.mp3", "Album A/02 Two.mp3"] {
        let t = file_tags(&r.dir.join(n));
        assert_eq!(
            (t.album.as_str(), t.album_artist.as_str(), t.year),
            ("Quokka Hits", "The Quokkas", 1999),
            "{n}"
        );
        assert_eq!(t.art.map(|a| a.data), Some(jpg.clone()), "{n}: the cover");
    }
    // The songs' own tags (titles, numbers) were not touched, and the library has them as one album again after the read.
    let albums: Vec<_> = r.app.library().albums().iter().filter(|a| a.title == "Quokka Hits").collect();
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].tracks.len(), 2);
    assert_eq!(r.app.library().search("quokka hits").albums.len(), 1);
    // Not a picture: refused with a message, and the choice is not made.
    let id = r.track_id("01 One.mp3");
    r.act(rvp_ui::Action::Lib(LibAction::EditTags(Scope::Track(id))));
    r.app.cover_picked("notes.txt", b"hello".to_vec(), 0);
    assert_eq!(r.form()["cover"], "Keep");
    // Remove the cover from one song.
    let remove = r.form()["buttons"].as_array().unwrap().iter().find(|b| b["id"] == 3).unwrap().clone();
    let (x, y) = (
        remove["rect"]["x"].as_f64().unwrap() as f32 + 10.0,
        remove["rect"]["y"].as_f64().unwrap() as f32 + 10.0,
    );
    for ev in [
        InputEvent::PointerMove { x, y },
        InputEvent::PointerDown { x, y, button: rvp_host::PointerButton::Primary },
        InputEvent::PointerUp { x, y, button: rvp_host::PointerButton::Primary },
    ] {
        r.host.input.0.push_back(ev);
        r.app.pump(&mut r.host);
    }
    r.run(50);
    assert_eq!(r.form()["cover"], "Remove");
    r.save();
    assert!(file_tags(&r.dir.join("Album A/01 One.mp3")).art.is_none());
    assert!(file_tags(&r.dir.join("Album A/02 Two.mp3")).art.is_some(), "the other song keeps its cover");
}

#[test]
fn flac_and_opus_songs_are_edited_too() {
    if skip() {
        return;
    }
    let mut r = Rig::with_fs("flac");
    for name in ["03 Three.flac", "04 Four.opus"] {
        let id = r.track_id(name);
        r.act(rvp_ui::Action::Lib(LibAction::EditTags(Scope::Track(id))));
        r.type_in("title", &format!("Edited {name}"));
        r.save();
        let sub = if name.contains("Three") { "Album B/03 Three.flac" } else { "Album B/04 Four.opus" };
        assert_eq!(file_tags(&r.dir.join(sub)).title, format!("Edited {name}"));
        assert_eq!(r.app.library().track(r.track_id(name)).unwrap().title, format!("Edited {name}"));
    }
}

#[test]
fn a_write_that_fails_or_a_file_that_cannot_be_edited_leaves_the_file_as_it_was() {
    if skip() {
        return;
    }
    // A write the host refuses, scripted.
    let mut w = ScriptedWriter::default();
    w.fail.insert(format!("{ROOT}/Album A/01 One.mp3"), "the disk is full".into());
    let mut r = Rig::new("fail", Some(Box::new(w)));
    let id = r.track_id("01 One.mp3");
    let old_title = r.app.library().track(id).unwrap().title.clone();
    r.act(rvp_ui::Action::Lib(LibAction::EditTags(Scope::Track(id))));
    r.type_in("title", "Never Written");
    r.save();
    assert_eq!(
        r.app.library().track(id).unwrap().title,
        old_title,
        "the library shows only what was written"
    );
    assert_eq!(file_tags(&r.dir.join("Album A/01 One.mp3")).title, old_title);
    // A file that is not what its name says: refused by the writer crate, nothing written.
    let bad = r.dir.join("Album A/02 Two.mp3");
    std::fs::write(&bad, b"this is not an mp3 at all, it is a note").unwrap();
    r.relist();
    r.run(3000);
    let before = std::fs::read(&bad).unwrap();
    let id2 = r.app.library().all_tracks().iter().find(|t| t.path.ends_with("02 Two.mp3")).map(|t| t.id);
    if let Some(id2) = id2 {
        r.act(rvp_ui::Action::Lib(LibAction::EditTags(Scope::Track(id2))));
        if r.app.ui().tag_form_open() {
            r.type_in("title", "x");
            r.save();
        }
    }
    assert_eq!(std::fs::read(&bad).unwrap(), before);
    // A file that went missing between the scan and the save.
    let mut r = Rig::with_fs("gone");
    let id = r.track_id("03 Three.flac");
    r.act(rvp_ui::Action::Lib(LibAction::EditTags(Scope::Track(id))));
    std::fs::remove_file(r.dir.join("Album B/03 Three.flac")).unwrap();
    r.type_in("title", "Gone");
    r.save();
    assert!(!r.dir.join("Album B/03 Three.flac").exists(), "nothing was created in its place");
    assert!(
        !r.dir.join("Album B").read_dir().unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("Gone"))
    );
}

#[test]
fn a_host_that_cannot_write_hides_the_editor_and_one_that_refuses_a_folder_says_why() {
    if skip() {
        return;
    }
    // No capability: the menu has no Edit tags and the key does nothing; asking anyway opens a read-only note.
    let mut r = Rig::new("nowrite", None);
    assert!(!r.model().app.tags);
    let id = r.track_id("01 One.mp3");
    let ctx_menu: Vec<String> =
        rvp_ui::actions::context_menu(r.model()).iter().map(|m| m.label.clone()).collect();
    assert!(!ctx_menu.iter().any(|l| l.contains("Edit tags")));
    r.act(rvp_ui::Action::Lib(LibAction::EditTags(Scope::Track(id))));
    let f = r.form();
    assert!(f["read_only"].as_str().unwrap().contains("cannot change files"), "{f}");
    let before = r.app.library().track(id).unwrap().title.clone();
    r.key(Key::Char('x'));
    r.key(Key::Enter); // Enter closes it
    assert!(!r.app.ui().tag_form_open());
    assert_eq!(r.app.library().track(id).unwrap().title, before);
    // A host that can write but refuses this folder (a browser: no permission) says so in the editor.
    let mut w = ScriptedWriter::default();
    w.refuse = Some("This folder was added without write access.".into());
    let mut r = Rig::new("refuse", Some(Box::new(w)));
    let id = r.track_id("01 One.mp3");
    r.act(rvp_ui::Action::Lib(LibAction::EditTags(Scope::Track(id))));
    assert_eq!(r.form()["read_only"], "This folder was added without write access.");
    r.key(Key::Char('x')); // typing changes nothing
    assert_eq!(r.field("title"), r.app.library().track(id).unwrap().title);
}
