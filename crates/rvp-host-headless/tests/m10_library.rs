//! M10: the library. A generated collection (about 200 tracks in 20 albums, mixed formats, with and without art, Unicode and odd
//! or missing tags: `tools/gen-library.py`) is scanned through the real demuxers and must come out as the exact artist, album and
//! track tree the generator wrote down (`expected.json`), with the right art on the right albums. A rescan after adding,
//! changing and deleting files reads only what changed. The index, the thumbnails and the playlists survive a restart.
use rvp_host::ScriptedLibrary;
use rvp_host_headless::{HeadlessHost, walk_listing};
use rvp_library::{Library, ListFormat, ScanEvent, Scanner, Thumb, art_key, encode_thumb};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures_dir() -> PathBuf {
    std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root().join("target/fixtures"))
}

fn skip() -> bool {
    std::env::var_os("RVP_SKIP_FIXTURES").is_some()
}

fn library_dir() -> PathBuf {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let st = Command::new("bash")
            .arg(root().join("tools/gen-fixtures.sh"))
            .arg(fixtures_dir())
            .env("RVP_FIXTURE_SET", "library")
            .status()
            .expect("run tools/gen-fixtures.sh");
        assert!(
            st.success(),
            "library fixture generation failed (ffmpeg and python3 needed; RVP_SKIP_FIXTURES=1 skips)"
        );
    });
    fixtures_dir().join("library")
}

fn expected() -> Value {
    serde_json::from_slice(&std::fs::read(library_dir().join("expected.json")).unwrap()).unwrap()
}

/// Scan `dir` into `lib` until the scanner is done.
fn scan(lib: &mut Library, dir: &Path) -> rvp_library::ScanReport {
    let mut host = HeadlessHost::new();
    let mut sc = ScriptedLibrary::default();
    sc.listings.push_back(walk_listing("music", "music", dir));
    host.library = Some(sc);
    let listing = host.library.as_mut().unwrap().listings.pop_front().unwrap();
    let mut scanner = Scanner::new();
    scanner.push(listing);
    let clock = host.virtual_clock();
    for _ in 0..100_000 {
        match scanner.tick(lib, &mut host) {
            ScanEvent::Finished(r) => return r,
            _ => clock.advance(1_000),
        }
    }
    panic!("the scan did not finish");
}

/// The tree as `(artist, album) -> [(path, title, artist, track)]` from the expected file.
type Tree = BTreeMap<(String, String), Vec<(String, String, String, u64)>>;

fn expected_tree(e: &Value) -> Tree {
    let mut m = BTreeMap::new();
    for a in e["albums"].as_array().unwrap() {
        let key = (a["artist"].as_str().unwrap().to_string(), a["album"].as_str().unwrap().to_string());
        let tracks = a["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                (
                    t["path"].as_str().unwrap().to_string(),
                    t["title"].as_str().unwrap().to_string(),
                    t["artist"].as_str().unwrap().to_string(),
                    t["track"].as_u64().unwrap_or(0),
                )
            })
            .collect();
        assert!(m.insert(key, tracks).is_none());
    }
    m
}

fn actual_tree(l: &Library) -> Tree {
    l.albums()
        .iter()
        .map(|a| {
            let tracks = a
                .tracks
                .iter()
                .map(|&id| {
                    let t = l.track(id).unwrap();
                    (
                        t.path.clone(),
                        t.display_title().to_string(),
                        t.display_artist().to_string(),
                        t.track_no as u64,
                    )
                })
                .collect();
            ((a.artist.clone(), a.title.clone()), tracks)
        })
        .collect()
}

#[test]
fn a_generated_library_scans_into_the_expected_tree() {
    if skip() {
        return;
    }
    let e = expected();
    let mut lib = Library::new();
    let t0 = std::time::Instant::now();
    let report = scan(&mut lib, &library_dir().join("music"));
    eprintln!("scanned {} tracks in {:?}: {report:?}", lib.track_count(), t0.elapsed());
    assert_eq!(report.failed, 0);
    assert_eq!(report.added, e["track_count"].as_u64().unwrap() as usize);
    assert_eq!(lib.track_count(), 201);
    assert_eq!(
        lib.albums().len(),
        20,
        "{:?}",
        lib.albums()
            .iter()
            .map(|a| format!("{} / {} ({})", a.artist, a.title, a.tracks.len()))
            .collect::<Vec<_>>()
    );

    // The tree: every album, with its tracks in order.
    let want = expected_tree(&e);
    let got = actual_tree(&lib);
    assert_eq!(got.keys().collect::<Vec<_>>(), want.keys().collect::<Vec<_>>(), "album list");
    for (k, tracks) in &want {
        assert_eq!(&got[k], tracks, "tracks of {k:?}");
    }

    // Artists: one per album artist, and their album counts.
    let mut artists: BTreeMap<String, usize> = BTreeMap::new();
    for (a, _) in want.keys() {
        *artists.entry(a.clone()).or_default() += 1;
    }
    assert_eq!(lib.artists().len(), artists.len());
    for a in lib.artists() {
        assert_eq!(a.albums.len(), artists[&a.name], "{}", a.name);
    }
    // Sorted without "The", Unknown Artist among the others.
    let names: Vec<&str> = lib.artists().iter().map(|a| a.name.as_str()).collect();
    let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
    assert!(pos("A Tribe of Pines") < pos("Aurora Vale"));
    assert!(pos("The Midnight Owls") < pos("Mono Mimi") && pos("Mono Mimi") < pos("Old Tapes"), "{names:?}");
    assert!(pos("Ünal Çelik") < pos("Unknown Artist"), "accents fold: {names:?}");

    // Years, durations, codecs.
    for a in e["albums"].as_array().unwrap() {
        let al = lib
            .albums()
            .iter()
            .find(|x| x.artist == a["artist"].as_str().unwrap() && x.title == a["album"].as_str().unwrap())
            .unwrap();
        assert_eq!(al.year as i64, a["year"].as_i64().unwrap_or(0), "{}", al.title);
        for (id, t) in al.tracks.iter().zip(a["tracks"].as_array().unwrap()) {
            let tr = lib.track(*id).unwrap();
            let want_us = (t["duration_s"].as_f64().unwrap() * 1e6) as i64;
            assert!(
                (tr.duration_us - want_us).abs() < 80_000,
                "{}: {} us, want about {want_us}",
                tr.path,
                tr.duration_us
            );
            let fmt = t["format"].as_str().unwrap();
            let codec_ok = match fmt {
                "mp3" => tr.codec == "mp3",
                "flac" => tr.codec == "flac",
                "opus" => tr.codec == "opus",
                "ogg" => tr.codec == "vorbis",
                "m4a" => tr.codec == "aac",
                "wav" => tr.codec.starts_with("pcm"),
                _ => false,
            };
            assert!(codec_ok, "{}: codec {}", tr.path, tr.codec);
            if let Some(d) = t["disc"].as_u64() {
                assert_eq!(tr.disc_no as u64, d, "{}", tr.path);
            }
        }
    }

    // Art: the right colour on the right albums (embedded or from the folder), none where there is none.
    for a in e["albums"].as_array().unwrap() {
        let al = lib
            .albums()
            .iter()
            .find(|x| x.artist == a["artist"].as_str().unwrap() && x.title == a["album"].as_str().unwrap())
            .unwrap();
        match a["art"].as_str() {
            None => {
                assert_eq!(al.art, 0, "{} should have no art", al.title);
                assert!(al.tracks.iter().all(|&t| lib.track(t).unwrap().art == 0));
            }
            Some(kind) => {
                assert_ne!(al.art, 0, "{} should have {kind} art", al.title);
                let th = lib.thumb(al.art).expect("thumbnail");
                let avg = th.average();
                let rgb: Vec<i64> =
                    a["art_rgb"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
                for k in 0..3 {
                    assert!(
                        (avg[k] as i64 - rgb[k]).abs() <= 8,
                        "{}: art colour {avg:?}, want {rgb:?}",
                        al.title
                    );
                }
                assert!(th.w.max(th.h) as u32 <= rvp_library::THUMB_SIDE);
                for &id in &al.tracks {
                    let t = lib.track(id).unwrap();
                    assert_eq!(t.art, al.art, "{}", t.path);
                    assert_eq!(t.art_embedded, kind == "embedded", "{}", t.path);
                }
            }
        }
    }
    // Eleven albums share nothing: one thumbnail per picture.
    assert_eq!(
        lib.used_art().len(),
        e["albums"].as_array().unwrap().iter().filter(|a| !a["art"].is_null()).count()
    );
    assert_eq!(report.folder_art_read, 3, "only the three folder pictures were read");
}

#[test]
fn search_finds_unicode_names_without_accents() {
    if skip() {
        return;
    }
    let mut lib = Library::new();
    scan(&mut lib, &library_dir().join("music"));
    assert_eq!(lib.search("zoe perez").artists.len(), 1);
    assert_eq!(lib.search("istanbul").albums.len(), 1);
    assert_eq!(lib.search("sigurdur thorsson").artists.len(), 1);
    assert_eq!(lib.search("\u{6771}\u{4eac}").artists.len(), 1);
    assert_eq!(lib.search("tribe pines").artists.len(), 1);
    assert!(lib.search("cafe").tracks.len() > 3);
    assert!(lib.search("zzzzqqq").tracks.is_empty());
}

fn copy_library() -> PathBuf {
    // A copy to change: the same place every run, overwritten in place (nothing is ever deleted recursively).
    let dst = fixtures_dir().join("library-rescan");
    let src = library_dir().join("music");
    fn copy(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).unwrap();
        for e in std::fs::read_dir(src).unwrap() {
            let e = e.unwrap();
            let to = dst.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy(&e.path(), &to);
            } else {
                std::fs::copy(e.path(), &to).unwrap();
            }
        }
    }
    // A failed earlier run may have left its additions behind.
    let _ = std::fs::remove_file(dst.join("Aurora Vale/Polar Nights/99 Brand New.mp3"));
    copy(&src, &dst);
    dst
}

#[test]
fn a_rescan_reads_only_what_was_added_changed_or_deleted() {
    if skip() {
        return;
    }
    let dir = copy_library();
    let src = library_dir().join("music");
    let mut lib = Library::new();
    scan(&mut lib, &dir);
    let all: Vec<_> = lib.all_tracks().to_vec();
    let id_of = |l: &Library, p: &str| l.all_tracks().iter().find(|t| t.path == p).map(|t| t.id);

    // Nothing changed: nothing is read.
    let r = scan(&mut lib, &dir);
    assert_eq!((r.added, r.changed, r.removed, r.unchanged), (0, 0, 0, 201), "{r:?}");

    // Add one file (a copy of a Polar Nights track under a new name), replace one (a different file's bytes under an old
    // name) and delete one.
    let polar = dir.join("Aurora Vale/Polar Nights");
    let a_track = std::fs::read_dir(&polar)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "mp3"))
        .unwrap();
    let added = polar.join("99 Brand New.mp3");
    std::fs::copy(&a_track, &added).unwrap();
    let mono = "Mono Mimi/Lofi Sketches";
    let mono_first = std::fs::read_dir(dir.join(mono)).unwrap().map(|e| e.unwrap().path()).min().unwrap();
    let changed_rel = format!("{mono}/{}", mono_first.file_name().unwrap().to_string_lossy());
    let changed_id = id_of(&lib, &changed_rel).unwrap();
    let donor = src.join("Delta Hiss/Feedback Loop");
    let donor_file = std::fs::read_dir(&donor).unwrap().map(|e| e.unwrap().path()).min().unwrap();
    std::fs::write(&mono_first, std::fs::read(&donor_file).unwrap()).unwrap();
    let gone =
        std::fs::read_dir(dir.join("Old Tapes/Side A")).unwrap().map(|e| e.unwrap().path()).min().unwrap();
    let gone_rel = format!("Old Tapes/Side A/{}", gone.file_name().unwrap().to_string_lossy());
    std::fs::remove_file(&gone).unwrap();

    let r = scan(&mut lib, &dir);
    assert_eq!((r.added, r.changed, r.removed, r.unchanged), (1, 1, 1, 199), "{r:?}");
    assert_eq!(lib.track_count(), 201);
    assert_eq!(id_of(&lib, &changed_rel), Some(changed_id), "a changed file keeps its id");
    assert!(id_of(&lib, &gone_rel).is_none());
    let new_id = id_of(&lib, "Aurora Vale/Polar Nights/99 Brand New.mp3").expect("the new file");
    assert!(all.iter().all(|t| t.id != new_id), "a new id");
    // Everything else is as it was.
    for t in &all {
        if t.path == gone_rel || t.path == changed_rel {
            continue;
        }
        let now = lib.track(t.id).unwrap_or_else(|| panic!("{} vanished", t.path));
        assert_eq!(now.title, t.title);
        assert_eq!(now.art, t.art, "{}", t.path);
    }
    // The changed file now carries the donor's tags (it moved to the donor's album) and the first album lost a track.
    let moved = lib.track(changed_id).unwrap();
    assert_eq!(moved.album, "Feedback Loop");
    let owl_albums = lib.albums().iter().filter(|a| a.title == "Lofi Sketches").count();
    assert_eq!(owl_albums, 1, "the album still exists with its other tracks");
    let old_tapes = lib.albums().iter().find(|a| a.title == "Side A").unwrap();
    assert_eq!(old_tapes.tracks.len(), 5);

    // Put the directory back the way it was for the next run.
    std::fs::remove_file(&added).unwrap();
    std::fs::copy(src.join(&changed_rel), &mono_first).unwrap();
    std::fs::copy(src.join(&gone_rel), &gone).unwrap();
}

#[test]
fn the_index_thumbnails_and_playlists_survive_a_restart() {
    if skip() {
        return;
    }
    let dir = library_dir().join("music");
    let mut lib = Library::new();
    scan(&mut lib, &dir);
    // A playlist made from the library, then exported and imported again.
    let id = lib.create_playlist("Road");
    let some: Vec<u32> = lib.albums()[3].tracks.iter().copied().take(4).collect();
    lib.playlist_add(id, &some);
    let m3u = lib.export_playlist(id, ListFormat::M3u8).unwrap();
    let pls = lib.export_playlist(id, ListFormat::Pls).unwrap();
    for text in [&m3u, &pls] {
        let again = lib.import_playlist("Again", text, "");
        assert_eq!(lib.playlist(again).unwrap().entries, lib.playlist(id).unwrap().entries);
    }

    // Save everything the way the app does (the index, each new thumbnail, the playlists), as bytes in a map.
    let mut store: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    store.insert(rvp_library::INDEX_KEY.into(), lib.save_index());
    for (aid, th) in lib.take_unsaved_art() {
        store.insert(art_key(aid), encode_thumb(&th));
    }
    store.insert(rvp_library::PLAYLISTS_KEY.into(), lib.save_playlists());
    assert!(!lib.index_dirty() && !lib.playlists_dirty());

    // A new session: load, ask for the thumbnails the tracks use, load the playlists.
    let mut l2 = Library::load_index(&store[rvp_library::INDEX_KEY]).unwrap();
    assert_eq!(l2.albums().len(), 20);
    assert!(l2.thumb(l2.albums()[0].art).is_none(), "thumbnails are loaded on demand");
    for a in l2.wanted_art() {
        assert!(l2.load_thumb(a, &store[&art_key(a)]));
    }
    assert!(l2.wanted_art().is_empty());
    assert_eq!(
        l2.thumb(lib.albums()[0].art).map(|t: &Thumb| t.rgb.clone()),
        lib.thumb(lib.albums()[0].art).map(|t| t.rgb.clone())
    );
    l2.load_playlists(&store[rvp_library::PLAYLISTS_KEY]).unwrap();
    assert_eq!(l2.playlists().len(), lib.playlists().len());
    assert_eq!(l2.playlist(id).unwrap().track_ids(), some);
    assert_eq!(actual_tree(&l2), actual_tree(&lib));

    // The first rescan after a restart reads nothing: the index knows every file.
    let r = scan(&mut l2, &dir);
    assert_eq!((r.added, r.changed, r.removed, r.unchanged), (0, 0, 0, 201), "{r:?}");
    assert_eq!(r.folder_art_read, 0, "folder pictures are remembered too");
    assert!(
        l2.all_tracks().iter().all(|t| !t.src.is_empty()),
        "files can be opened again once the root is listed"
    );
}
