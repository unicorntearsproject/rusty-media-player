//! Subtitles beyond plain text: a cue that is already on screen when a forward seek lands, ASS/SSA (styles, positions) in
//! Matroska and as a sidecar file, and PGS bitmap subtitles in Matroska against ffmpeg's rendering of them.
use rvp_host_headless::{FileSource, HeadlessHost, SessionState};
use rvp_player::Session;
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

/// A fixture path in the M8 set (`target/fixtures/m8/<name>`).
fn fixture(name: &str) -> String {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/gen-fixtures.sh");
        let st = Command::new("bash")
            .arg(script)
            .arg(dir())
            .env("RVP_FIXTURE_SET", "m8")
            .status()
            .expect("run tools/gen-fixtures.sh");
        assert!(st.success(), "fixture generation failed (is ffmpeg installed? RVP_SKIP_FIXTURES=1 skips)");
    });
    dir().join("m8").join(name).to_string_lossy().into_owned()
}

struct Rig {
    host: HeadlessHost,
    session: Session,
}

impl Rig {
    /// Open `path`, select its first subtitle track and start playing.
    fn new(path: &str) -> Self {
        let host = HeadlessHost::new();
        let codecs = rvp_host_headless::DefaultCodecs::default();
        let session = Session::new(FileSource::open(path).unwrap(), Rc::new(codecs));
        let mut r = Rig { host, session };
        r.run_until(|r| r.session.state() != SessionState::Opening, 3_000_000);
        let id = r.session.subtitle_tracks().first().expect("a subtitle track").id;
        r.session.select_subtitle(Some(id));
        r.session.play();
        r
    }

    /// Open `path` and load a sidecar subtitle file next to it.
    fn with_sidecar(path: &str, sub: &str) -> Self {
        let host = HeadlessHost::new();
        let codecs = rvp_host_headless::DefaultCodecs::default();
        let mut session = Session::new(FileSource::open(path).unwrap(), Rc::new(codecs));
        session.add_subtitle_source(FileSource::open(sub).unwrap(), "sub.ass");
        session.play();
        let mut r = Rig { host, session };
        r.run_until(|r| r.session.state() != SessionState::Opening, 3_000_000);
        r.settle(100_000);
        r
    }

    fn now(&self) -> i64 {
        rvp_host::HostClock::now_us(&*self.host.virtual_clock())
    }

    fn pos(&self) -> i64 {
        self.session.position_us(self.now())
    }

    /// Tick in 5 ms steps of virtual time until `done` holds; panics on timeout.
    fn run_until(&mut self, mut done: impl FnMut(&Rig) -> bool, limit_us: i64) {
        let clock = self.host.virtual_clock();
        let start = self.now();
        loop {
            self.session.tick(&mut self.host);
            if done(self) {
                return;
            }
            assert!(self.now() - start < limit_us, "timed out (state {:?})", self.session.state());
            clock.advance(5_000);
        }
    }

    fn settle(&mut self, us: i64) {
        let end = self.now() + us;
        let clock = self.host.virtual_clock();
        while self.now() < end {
            self.session.tick(&mut self.host);
            clock.advance(5_000);
        }
    }

    /// Play until the position reaches `t`, then one more tick so the subtitle state is current.
    fn play_to(&mut self, t: i64) {
        self.run_until(|r| r.pos() >= t, 60_000_000);
        self.settle(20_000);
    }

    /// Seek to `to`, play on until `to + after`, and say what is on screen then.
    fn seek_and_look(&mut self, to: i64, after: i64) -> Option<String> {
        self.session.seek(to);
        self.play_to(to + after);
        self.session.subtitle_text().map(String::from)
    }
}

#[test]
fn a_cue_that_began_before_a_forward_seek_is_shown_after_it() {
    if skip() {
        return;
    }
    // 24 s, a keyframe every second, clusters of up to five seconds; "Long cue" is 8 s to 24 s, "Short" is 16 s to 17 s. The
    // demuxer has not read as far as 8 s when the seek comes (a fresh session each time), so the cue is only found by looking
    // back from the landing point.
    for name in ["subs_long.mkv", "subs_long_ass.mkv", "subs_long.mp4"] {
        // (The MP4 file has the long cue only: its samples cannot overlap.)
        let at_16_4 = if name.ends_with(".mp4") { "Long cue" } else { "Long cue\nShort" };
        // Landing points: inside a cluster, at a keyframe, with the short cue on, just before the cue ends.
        for (to, want) in [
            (9_300_000, Some("Long cue")),
            (10_400_000, Some("Long cue")),
            (12_400_000, Some("Long cue")),
            (16_400_000, Some(at_16_4)),
            (17_900_000, Some("Long cue")),
            (23_500_000, Some("Long cue")),
            (7_800_000, None),
        ] {
            let mut rig = Rig::new(&fixture(name));
            rig.play_to(300_000);
            let got = rig.seek_and_look(to, 100_000);
            assert_eq!(got.as_deref(), want, "{name}: after a seek to {to} us");
            // And backwards to before the cue starts, then forwards again.
            assert_eq!(rig.seek_and_look(1_000_000, 100_000), None, "{name}: before the cue");
            assert_eq!(
                rig.seek_and_look(14_000_000, 100_000).as_deref(),
                Some("Long cue"),
                "{name}: forward again"
            );
        }
    }
}

#[test]
fn embedded_ass_in_matroska_keeps_its_styles_and_places() {
    if skip() {
        return;
    }
    let mut rig = Rig::new(&fixture("subs_ass.mkv"));
    assert_eq!(rig.session.subtitle_tracks().len(), 1, "the ASS track is listed");
    assert!(
        rig.session.warnings().iter().all(|w| !w.contains("not supported")),
        "{:?}",
        rig.session.warnings()
    );
    check_ass(&mut rig);
}

#[test]
fn a_sidecar_ass_file_is_the_same_as_the_embedded_one() {
    if skip() {
        return;
    }
    let mut rig = Rig::with_sidecar(&fixture("subs_long.mp4"), &fixture("sub.ass"));
    check_ass(&mut rig);
}

fn check_ass(rig: &mut Rig) {
    rig.play_to(1_500_000);
    let cues = rig.session.subtitle_cues().to_vec();
    assert_eq!(cues.len(), 1);
    assert_eq!(cues[0].text, "Hello");
    let r = cues[0].rich.as_ref().expect("styled");
    assert_eq!((r.align, r.pos, r.layer), (2, None, 0));
    assert_eq!(r.play_res, (320, 240));
    assert_eq!(r.lines[0][0].colour, 0xFFFF_FFFF, "white, opaque");
    assert_eq!(r.lines[0][0].size_permille, 20 * 1000 / 240, "20 on a 240 line script");

    // World in italics, two lines.
    rig.play_to(3_200_000);
    let cues = rig.session.subtitle_cues().to_vec();
    assert_eq!(cues.len(), 1);
    assert_eq!(cues[0].text, "World\ntwo lines");
    let r = cues[0].rich.as_ref().unwrap();
    assert_eq!(r.lines.len(), 2);
    assert!(r.lines[0][0].italic && !r.lines[0][0].bold);
    assert!(!r.lines[1][0].italic, "\\i0 ends the italics before the line break");

    // The corner cue joins it: layer 1, bold italic yellow style but green by override, top right at (300, 20).
    rig.play_to(3_700_000);
    let cues = rig.session.subtitle_cues().to_vec();
    assert_eq!(cues.len(), 2);
    assert_eq!(cues[0].text, "World\ntwo lines", "the lower layer is drawn first");
    assert_eq!(cues[1].text, "Corner");
    let r = cues[1].rich.as_ref().unwrap();
    assert_eq!((r.align, r.pos, r.layer), (9, Some((300, 20)), 1));
    let s = &r.lines[0][0];
    assert!(s.bold && s.italic, "from the style");
    assert_eq!(s.colour, 0x00FF_00FF, "\\c&H00FF00& is green");

    // Bold on and off inside a line, and a comma in the text.
    rig.play_to(5_200_000);
    let cues = rig.session.subtitle_cues().to_vec();
    assert_eq!(cues.len(), 1);
    assert_eq!(cues[0].text, "Last one, with a comma");
    let spans = &cues[0].rich.as_ref().unwrap().lines[0];
    assert_eq!((spans[0].text.as_str(), spans[0].bold), ("Last", true));
    assert_eq!((spans[1].text.as_str(), spans[1].bold), (" one, with a comma", false));

    rig.play_to(5_800_000);
    assert!(rig.session.subtitle_cues().is_empty());
}

/// ffmpeg's picture of the PGS stream at `t` seconds over black, as RGB24 (the oracle).
fn ffmpeg_overlay(path: &str, t: f64) -> Vec<u8> {
    let out = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=320x240:r=25:d=6,format=rgb24",
            "-i",
            path,
        ])
        .args(["-filter_complex", "[0:v][1:s:0]overlay=format=rgb", "-ss", &format!("{t}"), "-frames:v", "1"])
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()
        .expect("run ffmpeg");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.stdout.len(), 320 * 240 * 3);
    out.stdout
}

/// Our decode of the cues on screen composed over black as RGB24.
fn compose(cues: &[rvp_subs::Cue]) -> Vec<u8> {
    let mut px = vec![0u8; 320 * 240 * 3];
    for c in cues {
        let img = c.image.as_ref().expect("a picture cue");
        assert_eq!((img.width, img.height), (320, 240));
        for o in &img.objects {
            for y in 0..o.h {
                for x in 0..o.w {
                    let s = &o.rgba[((y * o.w + x) * 4) as usize..][..4];
                    let d = (((o.y + y) * 320 + o.x + x) * 3) as usize;
                    for k in 0..3 {
                        px[d + k] = ((s[k] as u32 * s[3] as u32 + 127) / 255) as u8;
                    }
                }
            }
        }
    }
    px
}

#[test]
fn pgs_bitmaps_in_matroska_match_ffmpegs_rendering() {
    if skip() {
        return;
    }
    let path = fixture("subs_pgs.mkv");
    let mut rig = Rig::new(&path);
    assert_eq!(rig.session.subtitle_tracks().len(), 1, "the PGS track is listed");
    for (t, shown) in [(0.5, false), (1.5, true), (2.7, false), (4.2, true), (5.7, false)] {
        rig.play_to((t * 1e6) as i64);
        let cues = rig.session.subtitle_cues().to_vec();
        assert_eq!(!cues.is_empty(), shown, "at {t} s");
        let (mine, theirs) = (compose(&cues), ffmpeg_overlay(&path, t));
        let worst = mine.iter().zip(&theirs).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(worst <= 4, "at {t} s: the picture differs from ffmpeg's by up to {worst}");
        if shown {
            assert!(theirs.iter().any(|&v| v > 100), "the oracle shows something");
        }
    }
    // Seeking into the middle of a picture's time shows it (it started before the landing keyframe).
    rig.session.seek(4_900_000);
    rig.play_to(5_000_000);
    assert_eq!(rig.session.subtitle_cues().len(), 1, "the picture of 4.0 s is still up at 5.0 s");
}
