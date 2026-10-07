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
    // The virtual display is needed unless the run is headed on purpose.
    let virt: &[&str] = if headed() { &[] } else { &["xvfb-run"] };
    let missing: Vec<&&str> = virt.iter().chain(tools).filter(|t| !have(t)).collect();
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

/// `rusty-wave` on a virtual display. Headless is the default and nothing may reach the real session: Wayland and the session type are
/// removed from the environment (winit would pick Wayland over the X display of Xvfb), and `xvfb-run` sets its own `DISPLAY`.
/// `RVP_HEADED=1` opts in to a visible window on the real display (it then needs no `xvfb-run`).
fn rvp(args: &[&str]) -> Command {
    let bin = env!("CARGO_BIN_EXE_rusty-wave");
    let mut c = if headed() {
        Command::new(bin)
    } else {
        let mut c = Command::new("xvfb-run");
        c.args(["-a", "-s", "-screen 0 1280x800x24", bin]);
        c
    };
    if !headed() {
        c.env_remove("WAYLAND_DISPLAY").env_remove("XDG_SESSION_TYPE").env("GDK_BACKEND", "x11");
    }
    // The first-run offers (default media player, the AppImage menu) are pop-ups that would eat the scripted keys, and the first run
    // adds the system's Music and Videos folders: neither may depend on (or touch) the real session. The user directories come from an
    // empty configuration folder unless a test points them somewhere (`with_user_dirs`).
    c.env("RVP_NO_OFFERS", "1").env("XDG_CONFIG_HOME", empty_config_home());
    c.args(args);
    c
}

/// An `XDG_CONFIG_HOME` without a `user-dirs.dirs`: the app finds no Music or Videos folder.
fn empty_config_home() -> PathBuf {
    static ONCE: Once = Once::new();
    let d = std::env::temp_dir().join(format!("rvp-desktop-test-xdg-empty-{}", std::process::id()));
    ONCE.call_once(|| std::fs::create_dir_all(&d).unwrap());
    d
}

/// Make `$XDG_CONFIG_HOME/user-dirs.dirs` (in a fresh folder under `dir`) name `music` and `videos` as the user's Music and Videos
/// folders; returns the config home to hand to the app.
fn with_user_dirs(dir: &Path, music: &Path, videos: &Path) -> PathBuf {
    let cfg = dir.join("xdg-config");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("user-dirs.dirs"),
        format!("XDG_MUSIC_DIR=\"{}\"\nXDG_VIDEOS_DIR=\"{}\"\n", music.display(), videos.display()),
    )
    .unwrap();
    cfg
}

/// `RVP_HEADED=1`: let the app open real windows (for looking at it by hand). Off by default.
fn headed() -> bool {
    std::env::var("RVP_HEADED").is_ok_and(|v| v == "1")
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
    let out = c.output().expect("start xvfb-run and rusty-wave");
    assert!(
        out.status.success(),
        "rusty-wave failed: {}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
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
    if skip("the playback smoke test", &["ffmpeg"]) {
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
    if skip("the library smoke test", &["ffmpeg"]) {
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
        "8", // the scan of 200 tracks takes a few seconds, more on a loaded machine
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
    if skip("the restart test", &["ffmpeg"]) {
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

#[test]
fn the_audio_settings_are_changed_from_the_keyboard_and_kept_for_the_next_run() {
    let _turn = one_at_a_time();
    if skip("the audio settings test", &[]) {
        return;
    }
    let dir = scratch("audio-settings");
    let data = dir.join("data");
    let (r1, r2) = (dir.join("r1.json"), dir.join("r2.json"));
    let base = |report: &Path| -> Vec<String> {
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
    // First run: U opens the panel; Space turns the crossfade on, Tab and Right make it 6 s, Down and Space turn the automatic level
    // on, Down and Left aim it a LUFS lower, Down and Right level by album; Escape closes.
    let mut a = base(&r1);
    for (t, k) in [
        ("1", "u"),
        ("1.5", "Space"),
        ("2", "Tab"),
        ("2.5", "Right"),
        ("3", "Down"),
        ("3.4", "Space"),
        ("3.8", "Down"),
        ("4.2", "Left"),
        ("4.6", "Down"),
        ("5", "Right"),
        ("5.4", "Escape"),
    ] {
        a.extend(["--press".to_string(), format!("{t}:{k}")]);
    }
    a.extend(["--exit-after".into(), "7".into()]);
    run_ok(rvp(&a.iter().map(String::as_str).collect::<Vec<_>>()));
    let first = std::fs::read_to_string(&r1).unwrap();
    assert!(first.contains("\"crossfade\": true"), "{first}");
    assert_eq!(num(&first, "crossfade_secs"), Some(6.0), "{first}");
    assert!(first.contains("\"auto_level\": true"), "{first}");
    assert_eq!(num(&first, "target_lufs"), Some(-15.0), "{first}");
    assert_eq!(text(&first, "level_mode").as_deref(), Some("album"), "{first}");
    // It is in the data directory...
    let saved = std::fs::read_to_string(data.join("settings%2faudio.bin"))
        .expect("settings/audio in the data directory");
    assert!(
        saved.contains("crossfade=1")
            && saved.contains("crossfade_secs=6")
            && saved.contains("level_mode=album"),
        "{saved}"
    );
    // ...and the next run starts with it, nothing pressed.
    let mut b = base(&r2);
    b.extend(["--exit-after".into(), "3".into()]);
    run_ok(rvp(&b.iter().map(String::as_str).collect::<Vec<_>>()));
    let second = std::fs::read_to_string(&r2).unwrap();
    assert!(second.contains("\"crossfade\": true"), "{second}");
    assert_eq!(num(&second, "crossfade_secs"), Some(6.0), "{second}");
    assert_eq!(num(&second, "target_lufs"), Some(-15.0), "{second}");
    assert_eq!(text(&second, "level_mode").as_deref(), Some("album"), "{second}");
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
    if skip("the MPRIS test", &["ffmpeg", "playerctl"]) {
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
        if let Some(n) = players().into_iter().find(|p| p.contains("RustyWave") && !before.contains(p)) {
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
    // The volume reads back what the player does, whoever set it: `playerctl volume 0.3` (the slider follows), and the player's own
    // keys are not covered here, the app test checks every source.
    playerctl(&name, &["volume", "0.3"]);
    std::thread::sleep(Duration::from_millis(600));
    let v: f64 = playerctl(&name, &["volume"]).parse().unwrap();
    assert!((v - 0.3).abs() < 0.01, "volume 0.3 reads back as {v}");
    playerctl(&name, &["stop"]);
    std::thread::sleep(Duration::from_millis(800));
    assert_ne!(playerctl(&name, &["status"]), "Playing");
    drop(child);
    std::fs::remove_dir_all(dir).ok();
}

/// Options every scripted run here shares: a data directory of its own, no sound, no media keys, a report.
fn base(data: &Path, report: &Path) -> Vec<String> {
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
}

fn run_args(mut c: Vec<String>, rest: &[&str]) {
    c.extend(rest.iter().map(|s| s.to_string()));
    run_ok(rvp(&c.iter().map(String::as_str).collect::<Vec<_>>()));
}

/// The names of the library folders in a report (`"library_roots": ["Music", "Videos"]`), sorted.
fn roots(report: &str) -> Vec<String> {
    let at =
        report.find("\"library_roots\":").expect("library_roots in the report") + "\"library_roots\":".len();
    let rest = report[at..].trim_start().strip_prefix('[').expect("a list");
    let list = &rest[..rest.find(']').unwrap()];
    let mut names: Vec<String> =
        list.split(',').map(|s| s.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty()).collect();
    names.sort(); // the library keeps them in the order of their ids
    names
}

#[test]
fn the_first_run_lands_on_the_library_and_the_next_run_stays_there() {
    let _turn = one_at_a_time();
    if skip("the first-run smoke test", &[]) {
        return;
    }
    let dir = scratch("first-run");
    let (data, r1, r2) = (dir.join("data"), dir.join("r1.json"), dir.join("r2.json"));
    run_args(base(&data, &r1), &["--exit-after", "2.5"]);
    let first = std::fs::read_to_string(&r1).unwrap();
    assert_eq!(text(&first, "mode").as_deref(), Some("library"), "{first}");
    assert!(first.contains("\"first_run\": true"), "{first}");
    assert!(first.contains("\"dialog\": null"), "no pop-up on the first run when offers are off\n{first}");
    assert!(roots(&first).is_empty(), "no Music or Videos folder to add\n{first}");
    assert_eq!(num(&first, "queue_len"), Some(0.0), "{first}");
    // The next run is not a first run, and opens on the face the last one ended on.
    run_args(base(&data, &r2), &["--exit-after", "2.5"]);
    let second = std::fs::read_to_string(&r2).unwrap();
    assert!(second.contains("\"first_run\": false"), "{second}");
    assert_eq!(text(&second, "mode").as_deref(), Some("library"), "{second}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn the_first_run_adds_the_music_and_videos_folders_once() {
    let _turn = one_at_a_time();
    if skip("the standard folders smoke test", &["ffmpeg"]) {
        return;
    }
    let fx = core_fixtures();
    let dir = scratch("std-folders");
    // The user's Music and Videos folders, with a little in them (the folder names are what the library shows).
    let (music, videos) = (dir.join("Music"), dir.join("Videos"));
    std::fs::create_dir_all(music.join("Test Artist")).unwrap();
    std::fs::create_dir_all(&videos).unwrap();
    for (i, (album, title)) in [("First", "One"), ("First", "Two"), ("Second", "Three")].iter().enumerate() {
        let f = music.join("Test Artist").join(format!("{i} {title}.flac"));
        let st = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=330:duration=1:sample_rate=44100",
            ])
            .args(["-metadata", &format!("title={title}"), "-metadata", "artist=Test Artist"])
            .args(["-metadata", &format!("album={album}"), "-metadata", &format!("track={}", i + 1)])
            .arg(&f)
            .status()
            .unwrap();
        assert!(st.success());
    }
    std::fs::copy(fx.join("h264_aac.mp4"), videos.join("Clip.mp4")).unwrap();
    let cfg = with_user_dirs(&dir, &music, &videos);
    let (data, r1, r2, r3) =
        (dir.join("data"), dir.join("r1.json"), dir.join("r2.json"), dir.join("r3.json"));
    let run = |report: &Path| {
        let mut c = rvp(&[]);
        // `rvp(&[])` has no arguments of its own: add ours and point the user directories at the folders above.
        c.args(base(&data, report)).args(["--exit-after", "6"]).env("XDG_CONFIG_HOME", &cfg);
        run_ok(c);
        std::fs::read_to_string(report).unwrap()
    };
    // First run: both folders are added and listed.
    let first = run(&r1);
    assert!(first.contains("\"first_run\": true"), "{first}");
    assert_eq!(text(&first, "mode").as_deref(), Some("library"), "{first}");
    assert_eq!(roots(&first), ["Music", "Videos"], "{first}");
    assert_eq!(num(&first, "library_tracks"), Some(3.0), "{first}");
    assert_eq!(num(&first, "library_albums"), Some(2.0), "{first}");
    // The video folder is scanned too. (The app does not fold videos into the library yet, only the library crate does: until it does,
    // the count is 0; it must never be more than the one file.)
    assert!(num(&first, "library_videos").is_some_and(|n| n <= 1.0), "{first}");
    // Second run: the folders are not added again (no duplicates), and what was scanned is still there.
    let second = run(&r2);
    assert!(second.contains("\"first_run\": false"), "{second}");
    assert_eq!(roots(&second), ["Music", "Videos"], "{second}");
    assert_eq!(num(&second, "library_tracks"), Some(3.0), "{second}");
    assert_eq!(num(&second, "library_videos"), num(&first, "library_videos"), "{second}");
    // Folders the system names later are not picked up behind the user's back: the offer was made once.
    let (music2, videos2) = (dir.join("Music 2"), dir.join("Videos 2"));
    std::fs::create_dir_all(&music2).unwrap();
    std::fs::create_dir_all(&videos2).unwrap();
    with_user_dirs(&dir, &music2, &videos2);
    let third = run(&r3);
    assert_eq!(roots(&third), ["Music", "Videos"], "{third}");
    // And a user who removed them all does not get them back (the library is empty, the first-run step is done): covered by the app's
    // own tests (`setup.rs`); here the saved choice is what matters.
    let saved = std::fs::read_to_string(data.join("settings%2fsetup.bin"))
        .expect("settings/setup in the data directory");
    assert!(saved.contains("folders=done"), "{saved}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn ctrl_comma_opens_settings_and_the_theme_dialog_is_one_step_further() {
    let _turn = one_at_a_time();
    if skip("the settings smoke test", &[]) {
        return;
    }
    let dir = scratch("settings");
    let data = dir.join("data");
    let report = |n: &str| dir.join(format!("{n}.json"));
    // Ctrl+, on the Library face (where the first run starts).
    run_args(base(&data, &report("a")), &["--press", "1:ctrl+,", "--exit-after", "3"]);
    let a = std::fs::read_to_string(report("a")).unwrap();
    assert_eq!(text(&a, "dialog").as_deref(), Some("Settings"), "{a}");
    // Escape closes it.
    run_args(
        base(&data, &report("b")),
        &["--press", "1:ctrl+,", "--press", "2:Escape", "--exit-after", "3.5"],
    );
    let b = std::fs::read_to_string(report("b")).unwrap();
    assert!(b.contains("\"dialog\": null"), "{b}");
    // The keyboard starts on the Close button (the primary one, the last); Down wraps to the first control: the three switches, then Audio
    // settings, then Theme.
    run_args(
        base(&data, &report("c")),
        &[
            "--press",
            "1:ctrl+,",
            "--press",
            "1.5:Down",
            "--press",
            "2:Down",
            "--press",
            "2.5:Down",
            "--press",
            "3:Down",
            "--press",
            "3.5:Down",
            "--press",
            "4:Enter",
            "--exit-after",
            "5",
        ],
    );
    let c = std::fs::read_to_string(report("c")).unwrap();
    assert_eq!(text(&c, "dialog").as_deref(), Some("Theme"), "{c}");
    // The Player face too: B, then Ctrl+,.
    run_args(base(&data, &report("d")), &["--press", "1:b", "--press", "1.5:ctrl+,", "--exit-after", "3"]);
    let d = std::fs::read_to_string(report("d")).unwrap();
    assert_eq!(text(&d, "mode").as_deref(), Some("player"), "{d}");
    assert_eq!(text(&d, "dialog").as_deref(), Some("Settings"), "{d}");
    std::fs::remove_dir_all(dir).ok();
}

/// The bug of 1.0.0-rc6: the folders the first run found were never written down, so after a restart the library listed its albums and
/// posters from the saved index but could play nothing ("Those files aren't reachable right now"): every folder was disconnected. This runs
/// the whole story with a throwaway home: discover, play from the library, quit, start again with the same data folder and no
/// arguments, and play an album, a song and a film from the library.
#[test]
fn the_libraries_found_on_the_first_run_still_play_after_a_restart() {
    let _turn = one_at_a_time();
    if skip("the restart smoke test", &["ffmpeg"]) {
        return;
    }
    let fx = core_fixtures();
    let dir = scratch("restart-plays");
    let (music, videos) = (dir.join("Music"), dir.join("Videos"));
    std::fs::create_dir_all(music.join("Test Artist")).unwrap();
    std::fs::create_dir_all(&videos).unwrap();
    for (i, (album, title)) in [("First", "One"), ("First", "Two"), ("Second", "Three")].iter().enumerate() {
        let f = music.join("Test Artist").join(format!("{i} {title}.flac"));
        let st = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=330:duration=3:sample_rate=44100",
            ])
            .args(["-metadata", &format!("title={title}"), "-metadata", "artist=Test Artist"])
            .args(["-metadata", &format!("album={album}"), "-metadata", &format!("track={}", i + 1)])
            .arg(&f)
            .status()
            .unwrap();
        assert!(st.success());
    }
    std::fs::copy(fx.join("h264_aac.mp4"), videos.join("Clip.mp4")).unwrap();
    let cfg = with_user_dirs(&dir, &music, &videos);
    let data = dir.join("data");
    let run = |name: &str, keys: &[(&str, &str)]| {
        let report = dir.join(format!("{name}.json"));
        let mut c = rvp(&[]);
        c.args(base(&data, &report)).args(["--exit-after", "4.5"]).env("XDG_CONFIG_HOME", &cfg);
        for (t, k) in keys {
            c.args(["--press", &format!("{t}:{k}")]);
        }
        run_ok(c);
        std::fs::read_to_string(report).unwrap()
    };
    // Tracks view, first song.
    let tracks = [("2", "3"), ("2.6", "Down"), ("3", "Enter")];
    let first = run("first", &tracks);
    assert!(first.contains("\"first_run\": true"), "{first}");
    assert_eq!(num(&first, "library_roots_connected"), Some(2.0), "{first}");
    assert_eq!(
        text(&first, "state").as_deref(),
        Some("Playing"),
        "playing from the library on the first run: {first}"
    );
    // The restart, nothing on the command line: every folder is connected again, everything the index lists can be played, and the
    // library plays an album from its Albums view.
    let second =
        run("second", &[("2", "1"), ("2.6", "Down"), ("3", "Enter"), ("3.6", "Down"), ("4", "Enter")]);
    assert!(second.contains("\"first_run\": false"), "{second}");
    assert_eq!(
        num(&second, "library_roots_connected"),
        Some(2.0),
        "folders not reconnected after a restart: {second}"
    );
    assert_eq!(num(&second, "library_playable_tracks"), num(&second, "library_tracks"), "{second}");
    assert_eq!(num(&second, "library_tracks"), Some(3.0), "{second}");
    assert_eq!(num(&second, "library_playable_videos"), Some(1.0), "{second}");
    assert_eq!(
        text(&second, "state").as_deref(),
        Some("Playing"),
        "an album from the restarted library: {second}"
    );
    assert!(text(&second, "title").is_some_and(|t| !t.is_empty()), "{second}");
    // A song from the Tracks view, and the film from the Videos view (it opens on the Player face).
    let third = run("third", &tracks);
    assert_eq!(text(&third, "state").as_deref(), Some("Playing"), "a song after a restart: {third}");
    let film = run("film", &[("2", "8"), ("2.6", "Down"), ("3", "Enter")]);
    assert_eq!(text(&film, "state").as_deref(), Some("Playing"), "a film after a restart: {film}");
    assert_eq!(text(&film, "mode").as_deref(), Some("player"), "{film}");
    assert!(num(&film, "video_frames").is_some_and(|n| n > 0.0), "{film}");
    // A folder that is not there at a start (an unplugged drive) is remembered, not forgotten: it comes back when the folder does.
    let hidden = dir.join("Music-away");
    std::fs::rename(&music, &hidden).unwrap();
    let away = run("away", &[]);
    assert_eq!(num(&away, "library_roots_connected"), Some(1.0), "{away}");
    std::fs::rename(&hidden, &music).unwrap();
    let back = run("back", &[]);
    assert_eq!(
        num(&back, "library_roots_connected"),
        Some(2.0),
        "a remembered folder did not come back: {back}"
    );
    assert_eq!(
        num(&back, "library_playable_tracks"),
        Some(3.0),
        "the folder's songs are playable again: {back}"
    );
    std::fs::remove_dir_all(dir).ok();
}
