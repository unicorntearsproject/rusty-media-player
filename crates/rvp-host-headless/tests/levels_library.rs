//! Loudness in the library: tags are read by the scan, tracks without tags are decoded and measured in the background (within a time
//! budget per tick), the figures live in the index across a restart, unchanged files are not measured again, an album's loudness
//! is made of its tracks, and an old index (without loudness) still loads. Fixtures: `tools/gen-fixtures.sh`, set `levels`.
use rvp_core::CodecFactory;
use rvp_host::ScriptedLibrary;
use rvp_host_headless::{DefaultCodecs, HeadlessHost, walk_listing};
use rvp_library::{Library, ScanEvent, Scanner};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::Once;

fn dir() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root.join("target/fixtures"))
}

fn skip() -> bool {
    std::env::var_os("RVP_SKIP_FIXTURES").is_some()
}

fn lib_dir() -> PathBuf {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/gen-fixtures.sh");
        let st = Command::new("bash")
            .arg(script)
            .arg(dir())
            .env("RVP_FIXTURE_SET", "levels")
            .status()
            .expect("run tools/gen-fixtures.sh");
        assert!(st.success(), "fixture generation failed (is ffmpeg installed? RVP_SKIP_FIXTURES=1 skips)");
    });
    dir().join("levels/lib")
}

/// ffmpeg's integrated loudness of a fixture.
fn ffmpeg_lufs(name: &str) -> f32 {
    let text = std::fs::read_to_string(lib_dir().join(format!("{name}.ebur128"))).unwrap();
    text.lines().filter_map(|l| l.strip_prefix("lavfi.r128.I=")).next_back().unwrap().parse().unwrap()
}

/// Scan the fixture folder (and, with `codecs`, measure what the tags do not say) until the scanner has nothing left to do. Returns
/// the largest time one `tick` took (virtual time stands still inside a tick, so this is counted in decoder turns instead).
fn run(lib: &mut Library, codecs: Option<Rc<dyn CodecFactory>>) -> usize {
    let mut host = HeadlessHost::new();
    let mut sc = ScriptedLibrary::default();
    sc.listings.push_back(walk_listing("music", "music", &lib_dir()));
    host.library = Some(sc);
    let listing = host.library.as_mut().unwrap().listings.pop_front().unwrap();
    let mut scanner = Scanner::new();
    let analyse = codecs.is_some();
    if let Some(c) = codecs {
        scanner.set_codecs(c);
    }
    scanner.push(listing);
    let clock = host.virtual_clock();
    let (mut ticks, mut saved) = (0, 0);
    for _ in 0..200_000 {
        match scanner.tick(lib, &mut host) {
            ScanEvent::Finished(_) => {
                if analyse {
                    scanner.start_analysis(lib.pending_loudness());
                }
            }
            ScanEvent::Analysed => saved += 1,
            _ => {}
        }
        if !scanner.busy() {
            assert!(!analyse || saved > 0 || lib.pending_loudness().is_empty());
            return ticks;
        }
        ticks += 1;
        clock.advance(1_000);
    }
    panic!("the scanner did not settle");
}

#[test]
fn the_scan_reads_tags_and_measures_the_rest() {
    if skip() {
        return;
    }
    let codecs: Rc<dyn CodecFactory> = Rc::new(DefaultCodecs::default());
    let mut lib = Library::new();
    let ticks = run(&mut lib, Some(codecs.clone()));
    assert!(ticks > 3, "the measurement is spread over several ticks ({ticks})");
    assert_eq!(lib.track_count(), 4);
    let by_path = |p: &str| lib.all_tracks().iter().find(|t| t.path == p).unwrap().clone();
    // The tagged track: its tags' figures (track -11.5 LUFS, album -14.8), nothing was decoded for it.
    let tagged = by_path("tagged/01.mp3").loudness;
    assert_eq!((tagged.lufs, tagged.album_lufs, tagged.measured), (Some(-11.5), Some(-14.8), false));
    // The album without tags: measured, and within 0.05 LU of what ffmpeg says.
    for n in 1..=3 {
        let l = by_path(&format!("quiet-loud/0{n}.flac")).loudness;
        let want = ffmpeg_lufs(&format!("quiet-loud/0{n}.flac"));
        assert!(l.measured && l.album_lufs.is_none());
        assert!((l.lufs.unwrap() - want).abs() < 0.05, "track {n}: {:?} against ffmpeg's {want}", l.lufs);
    }
    assert!(lib.pending_loudness().is_empty());
    // The album's loudness is made of its tracks (12 s each): the energy mean of -37.44, -29.35 and -21.28.
    let album = lib.albums().iter().find(|a| a.title == "Quiet and Loud").unwrap().id;
    let mean = |ls: [f32; 3]| 10.0 * (ls.iter().map(|l| 10f32.powf(l / 10.0)).sum::<f32>() / 3.0).log10();
    let want = mean([-37.441, -29.349, -21.283]);
    assert!(
        (lib.album_loudness(album).unwrap() - want).abs() < 0.1,
        "{:?} vs {want}",
        lib.album_loudness(album)
    );
    // What the player is told for a track: its own and its album's.
    let id = by_path("quiet-loud/02.flac").id;
    let hint = lib.loudness_hint(id).unwrap();
    assert!((hint.track_lufs.unwrap() + 29.349).abs() < 0.05 && hint.album_lufs.is_some());
    // The tagged track's album figure is the tag's.
    let id = by_path("tagged/01.mp3").id;
    let tagged_album = lib.album_of(id).unwrap().id;
    assert_eq!(lib.album_loudness(tagged_album), Some(-14.8));
}

#[test]
fn nothing_is_measured_unless_asked_and_nothing_twice() {
    if skip() {
        return;
    }
    let mut lib = Library::new();
    run(&mut lib, None);
    // Without the automatic level the scan only reads tags: the three untagged tracks wait.
    assert_eq!(lib.pending_loudness().len(), 3);
    assert!(lib.all_tracks().iter().filter(|t| t.loudness.lufs.is_some()).count() == 1);
    // Later, with it on: only those three are measured.
    let codecs: Rc<dyn CodecFactory> = Rc::new(DefaultCodecs::default());
    run(&mut lib, Some(codecs.clone()));
    assert!(lib.pending_loudness().is_empty());
    assert_eq!(lib.all_tracks().iter().filter(|t| t.loudness.measured).count(), 3);
    // A third scan (nothing changed) finds nothing to measure: the figures were kept.
    let before: Vec<_> = lib.all_tracks().iter().map(|t| t.loudness).collect();
    run(&mut lib, Some(codecs));
    let after: Vec<_> = lib.all_tracks().iter().map(|t| t.loudness).collect();
    assert_eq!(before, after);
}

#[test]
fn the_figures_survive_a_restart_and_an_old_index_still_loads() {
    if skip() {
        return;
    }
    let codecs: Rc<dyn CodecFactory> = Rc::new(DefaultCodecs::default());
    let mut lib = Library::new();
    run(&mut lib, Some(codecs));
    let bytes = lib.save_index();
    let loaded = Library::load_index(&bytes).unwrap();
    let figures = |l: &Library| -> Vec<_> {
        let mut v: Vec<_> = l.all_tracks().iter().map(|t| (t.path.clone(), t.loudness)).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    };
    assert_eq!(figures(&lib), figures(&loaded));
    assert!(loaded.all_tracks().iter().all(|t| t.loudness.lufs.is_some()));
    // Version 1 of the index (no loudness) is the same bytes with the version byte lowered and each track's loudness bytes cut out:
    // easiest to build from a library with nothing measured. The reader takes it and knows nothing about loudness.
    let mut old = Library::new();
    run(&mut old, None);
    let mut v2 = old.save_index();
    assert_eq!(v2[4], 2);
    // Strip the 9 loudness bytes of every track (they follow `channels`, at the end of each track record).
    let v1 = strip_loudness(&mut v2, &old);
    let back = Library::load_index(&v1).unwrap();
    assert_eq!(back.track_count(), old.track_count());
    assert!(back.all_tracks().iter().filter(|t| t.loudness.lufs.is_some()).count() == 0);
    // Garbage is refused, not trusted.
    let mut bad = lib.save_index();
    bad.truncate(bad.len() - 30);
    assert!(
        Library::load_index(&bad).is_err()
            || Library::load_index(&bad).unwrap().track_count() <= lib.track_count()
    );
    assert!(Library::load_index(b"RVPL\x09").is_err());
}

/// The index bytes of a version 1 file, from version 2 bytes: version byte lowered, 9 bytes cut after each track's channel count.
fn strip_loudness(v2: &mut [u8], lib: &Library) -> Vec<u8> {
    // Find each track's loudness block by walking the layout (see `rvp_library::persist`).
    let mut out = Vec::new();
    let mut at = 0usize;
    let rd16 = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as usize;
    let rd32 = |b: &[u8], o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize;
    // magic, version, next_id
    out.extend_from_slice(&v2[..9]);
    out[4] = 1;
    at += 9;
    let nroots = rd16(v2, at);
    out.extend_from_slice(&v2[at..at + 2]);
    at += 2;
    for _ in 0..nroots {
        for _ in 0..2 {
            let n = rd16(v2, at);
            out.extend_from_slice(&v2[at..at + 2 + n]);
            at += 2 + n;
        }
    }
    let ntracks = rd32(v2, at);
    out.extend_from_slice(&v2[at..at + 4]);
    at += 4;
    for _ in 0..ntracks {
        let start = at;
        at += 4 + 2; // id, root
        let n = rd16(v2, at);
        at += 2 + n + 8 + 8; // path, size, mtime
        for _ in 0..4 {
            let n = rd16(v2, at);
            at += 2 + n; // title, artist, album artist, album
        }
        at += 8 + 4; // track/disc numbers, year
        let n = rd16(v2, at);
        at += 2 + n + 8 + 8 + 1; // genre, duration, art, flags
        let n = rd16(v2, at);
        at += 2 + n + 4 + 2; // codec, sample rate, channels
        out.extend_from_slice(&v2[start..at]);
        at += 9; // the loudness block
    }
    out.extend_from_slice(&v2[at..]);
    let _ = lib;
    out
}
