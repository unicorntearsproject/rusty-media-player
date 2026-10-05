#![cfg(unix)]
//! The desktop app, started for real on a virtual X display (`xvfb-run`): it opens a fixture, plays it, shows decoded frames, takes
//! its own screenshot and reports the audio clock; the library face lists a generated library (with Cyrillic and Japanese names); the
//! queue and position come back after a restart; and `playerctl` can read and drive it over MPRIS.
//!
//! Needs `xvfb-run` (and ffmpeg for the fixtures; `playerctl` and a D-Bus session for the MPRIS test). A test whose tools are missing
//! says so and passes; `RVP_REQUIRE_DESKTOP_TESTS=1` turns that into a failure (CI sets it).
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard, Once};
use std::time::{Duration, Instant};

/// The tests start whole applications and measure the clock: they take turns so they do not starve each other.
fn one_at_a_time() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures_dir() -> PathBuf {
    std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root().join("target/fixtures"))
}

fn have(tool: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// True (after saying so) when the tools for a test are missing.
fn skip(what: &str, tools: &[&str]) -> bool {
    let missing: Vec<&&str> = tools.iter().filter(|t| !have(t)).collect();
    if missing.is_empty() {
        return false;
    }
    assert!(
        std::env::var_os("RVP_REQUIRE_DESKTOP_TESTS").is_none(),
        "{what}: needs {missing:?} (RVP_REQUIRE_DESKTOP_TESTS is set)"
    );
    eprintln!("skipped {what}: {missing:?} not installed");
    true
}

fn fixtures(set: &str) {
    let st = Command::new("bash")
        .arg(root().join("tools/gen-fixtures.sh"))
        .arg(fixtures_dir())
        .env("RVP_FIXTURE_SET", set)
        .status()
        .expect("run tools/gen-fixtures.sh");
    assert!(st.success(), "fixture generation failed (ffmpeg and python3 needed)");
}

fn core_fixtures() -> PathBuf {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| fixtures("basic"));
    fixtures_dir()
}

fn library_fixtures() -> PathBuf {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| fixtures("library"));
    fixtures_dir().join("library/music")
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("rvp-desktop-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `rvp` on a virtual display.
fn rvp(args: &[&str]) -> Command {
    let mut c = Command::new("xvfb-run");
    c.args(["-a", "-s", "-screen 0 1280x800x24", env!("CARGO_BIN_EXE_rvp")]).args(args);
    c
}

/// A number from the report (`"key": 123`).
fn num(report: &str, key: &str) -> Option<f64> {
    let at = report.find(&format!("\"{key}\":"))? + key.len() + 3;
    let rest = report[at..].trim_start();
    let end = rest.find([',', '\n', '}']).unwrap_or(rest.len());
    rest[..end].trim().parse().ok()
}

fn text(report: &str, key: &str) -> Option<String> {
    let at = report.find(&format!("\"{key}\":"))? + key.len() + 3;
    let rest = report[at..].trim_start().strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_string())
}

fn run_ok(mut c: Command) {
    let out = c.output().expect("start xvfb-run and rvp");
    assert!(out.status.success(), "rvp failed: {}\n{}", out.status, String::from_utf8_lossy(&out.stderr));
}

fn read_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let dec = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(path).expect("a screenshot was written"),
    ));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

#[test]
fn it_plays_a_fixture_shows_frames_and_keeps_time() {
    let _turn = one_at_a_time();
    if skip("the playback smoke test", &["xvfb-run", "ffmpeg"]) {
        return;
    }
    let fx = core_fixtures();
    let dir = scratch("play");
    let (shot, report) = (dir.join("shot.png"), dir.join("report.json"));
    run_ok(rvp(&[
        "--data-dir",
        dir.join("data").to_str().unwrap(),
        "--no-audio",
        "--no-media-keys",
        "--window",
        "1280x720",
        "--exit-after",
        "5",
        "--screenshot",
        shot.to_str().unwrap(),
        "--screenshot-after",
        "4",
        "--report",
        report.to_str().unwrap(),
        fx.join("h264_aac.mp4").to_str().unwrap(),
    ]));
    let r = std::fs::read_to_string(&report).unwrap();
    assert_eq!(text(&r, "title").as_deref(), Some("h264_aac.mp4"), "{r}");
    assert!(r.contains("\"saw_playing\": true"), "{r}");
    let pos = num(&r, "max_position_ms").unwrap();
    assert!((3000.0..=6000.0).contains(&pos), "position {pos} ms after about 5 s\n{r}");
    // The clock keeps time: media time advances at 1.00 +- 5 % of the wall clock.
    let ratio = num(&r, "clock_ratio").expect("a clock ratio (playing for over a second)");
    assert!((0.95..=1.05).contains(&ratio), "clock ratio {ratio}\n{r}");
    assert!(num(&r, "video_frames").unwrap() >= 40.0, "{r}");
    assert!(r.contains("\"video_size\": [320, 240]"), "{r}");
    // The screenshot is the window: 1280x720, with the test pattern's saturated colours in it and the controls drawn.
    let (w, h, px) = read_png(&shot);
    assert_eq!((w, h), (1280, 720));
    let saturated = px
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| {
            let (mx, mn) = (p[..3].iter().max().unwrap(), p[..3].iter().min().unwrap());
            mx - mn > 150
        })
        .count();
    assert!(saturated > 150_000, "only {saturated} colourful pixels: the video is not on screen");
    assert!(px.as_chunks::<4>().0.iter().all(|p| p[3] == 255), "every pixel is opaque");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn the_library_face_scans_a_folder_and_shows_it() {
    let _turn = one_at_a_time();
    if skip("the library smoke test", &["xvfb-run", "ffmpeg"]) {
        return;
    }
    let music = library_fixtures();
    let dir = scratch("lib");
    let (shot, report) = (dir.join("shot.png"), dir.join("report.json"));
    run_ok(rvp(&[
        "--data-dir",
        dir.join("data").to_str().unwrap(),
        "--no-audio",
        "--no-media-keys",
        "--window",
        "1280x720",
        "--press",
        "1.5:b",
        "--press",
        "2.5:2",
        "--press",
        "3:End",
        "--exit-after",
        "5",
        "--screenshot",
        shot.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
        music.to_str().unwrap(),
    ]));
    let r = std::fs::read_to_string(&report).unwrap();
    assert_eq!(text(&r, "mode").as_deref(), Some("library"), "{r}");
    assert_eq!(num(&r, "library_tracks"), Some(201.0), "{r}");
    assert_eq!(num(&r, "library_albums"), Some(20.0), "{r}");
    // The scan was saved: the index is in the data directory for the next start.
    let saved: Vec<String> = std::fs::read_dir(dir.join("data"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(saved.iter().any(|f| f.starts_with("library%2findex")), "{saved:?}");
    assert!(saved.iter().filter(|f| f.starts_with("library%2fart")).count() >= 10, "{saved:?}");
    let (w, h, _) = read_png(&shot);
    assert_eq!((w, h), (1280, 720));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn the_queue_and_the_position_come_back_after_a_restart() {
    let _turn = one_at_a_time();
    if skip("the restart test", &["xvfb-run", "ffmpeg"]) {
        return;
    }
    let dir = scratch("restore");
    let long = dir.join("long.flac");
    let st = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=330:duration=60:sample_rate=44100"])
        .arg(&long)
        .status()
        .unwrap();
    assert!(st.success());
    let data = dir.join("data");
    let (r1, r2) = (dir.join("r1.json"), dir.join("r2.json"));
    let common = |report: &Path| -> Vec<String> {
        [
            "--data-dir",
            data.to_str().unwrap(),
            "--no-audio",
            "--no-media-keys",
            "--report",
            report.to_str().unwrap(),
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    };
    // First run: play for about six seconds, then quit.
    let mut a = common(&r1);
    a.extend(["--exit-after".into(), "7".into(), long.to_str().unwrap().into()]);
    run_ok(rvp(&a.iter().map(String::as_str).collect::<Vec<_>>()));
    let first = std::fs::read_to_string(&r1).unwrap();
    let at = num(&first, "position_ms").unwrap();
    assert!(at > 3000.0, "{first}");
    // Second run: nothing on the command line. The queue is back, paused, where it was.
    let mut b = common(&r2);
    b.extend(["--exit-after".into(), "4".into()]);
    run_ok(rvp(&b.iter().map(String::as_str).collect::<Vec<_>>()));
    let second = std::fs::read_to_string(&r2).unwrap();
    assert_eq!(num(&second, "queue_len"), Some(1.0), "{second}");
    assert_eq!(text(&second, "title").as_deref(), Some("long.flac"), "{second}");
    assert_eq!(text(&second, "state").as_deref(), Some("Paused"), "{second}");
    let back = num(&second, "position_ms").unwrap();
    assert!((back - at).abs() < 1500.0, "saved at {at} ms, restored at {back} ms\n{second}");
    std::fs::remove_dir_all(dir).ok();
}

/// A started `xvfb-run rvp` that is stopped, with everything it started, when the test ends (also when it fails).
struct Reaper(std::process::Child);

impl Drop for Reaper {
    fn drop(&mut self) {
        let pgid = self.0.id();
        let _ = Command::new("kill").args(["-TERM", "--", &format!("-{pgid}")]).status();
        let _ = self.0.wait();
    }
}

fn players() -> Vec<String> {
    let out = Command::new("playerctl").arg("-l").output();
    out.map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::to_string).collect()).unwrap_or_default()
}

fn playerctl(player: &str, args: &[&str]) -> String {
    let out = Command::new("playerctl").arg("-p").arg(player).args(args).output().expect("playerctl");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn playerctl_reads_and_drives_it_over_mpris() {
    let _turn = one_at_a_time();
    if skip("the MPRIS test", &["xvfb-run", "ffmpeg", "playerctl"]) {
        return;
    }
    let session_bus = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
        || std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|d| Path::new(&d).join("bus").exists());
    if !session_bus {
        eprintln!("skipped the MPRIS test: no D-Bus session bus");
        return;
    }
    let dir = scratch("mpris");
    let long = dir.join("Tagged Tone.flac");
    let st = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=330:duration=60:sample_rate=44100"])
        .args([
            "-metadata",
            "title=Tagged Tone",
            "-metadata",
            "artist=Test Artist",
            "-metadata",
            "album=Test Album",
        ])
        .arg(&long)
        .status()
        .unwrap();
    assert!(st.success());
    let before = players();
    let child = Reaper(
        rvp(&[
            "--data-dir",
            dir.join("data").to_str().unwrap(),
            "--no-audio",
            "--exit-after",
            "40",
            long.to_str().unwrap(),
        ])
        .process_group(0)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    let name = loop {
        if let Some(n) = players().into_iter().find(|p| p.contains("RustyVideoPlayer") && !before.contains(p))
        {
            break n;
        }
        assert!(Instant::now() < deadline, "rvp never showed up on the session bus: {:?}", players());
        std::thread::sleep(Duration::from_millis(250));
    };
    // Metadata and state.
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(playerctl(&name, &["metadata", "xesam:title"]), "Tagged Tone");
    assert_eq!(playerctl(&name, &["metadata", "xesam:artist"]), "Test Artist");
    assert_eq!(playerctl(&name, &["metadata", "xesam:album"]), "Test Album");
    assert_eq!(playerctl(&name, &["status"]), "Playing");
    let len: f64 = playerctl(&name, &["metadata", "mpris:length"]).parse().unwrap();
    assert!((59_000_000.0..=61_000_000.0).contains(&len), "{len}");
    // Pause and resume.
    playerctl(&name, &["pause"]);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(playerctl(&name, &["status"]), "Paused");
    let p1: f64 = playerctl(&name, &["position"]).parse().unwrap();
    std::thread::sleep(Duration::from_millis(800));
    let p2: f64 = playerctl(&name, &["position"]).parse().unwrap();
    assert!((p2 - p1).abs() < 0.2, "paused but moving: {p1} -> {p2}");
    // Seek.
    playerctl(&name, &["position", "30"]);
    std::thread::sleep(Duration::from_millis(800));
    let p3: f64 = playerctl(&name, &["position"]).parse().unwrap();
    assert!((29.0..=31.5).contains(&p3), "seek to 30 s landed at {p3}");
    playerctl(&name, &["play"]);
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(playerctl(&name, &["status"]), "Playing");
    playerctl(&name, &["stop"]);
    std::thread::sleep(Duration::from_millis(800));
    assert_ne!(playerctl(&name, &["status"]), "Playing");
    drop(child);
    std::fs::remove_dir_all(dir).ok();
}
