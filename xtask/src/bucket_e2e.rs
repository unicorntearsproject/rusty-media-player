//! `cargo xtask bucket-e2e`: scenarios that run the packed `Rusty Wave.bucket` in the Bucket Simulator (`../rust-os/tools/bucket-sim`),
//! headless, on its virtual clock, so every run is the same and takes about a second.
//!
//! What a scenario can see, since the simulator prints very little: the exit status, its own messages, the app's log (the adapter
//! writes trace lines, level 4, for every now-playing report, audio open, transport command, library listing and so on; the OS
//! prints them only with `--log-level 4`), the audio the first stream played (`--audio-capture`), the screenshots, and the files
//! the app left in its data directory. Screenshots land in `target/bucket-e2e/<scenario>/` (overwritten in place) so a person can
//! look at them; the data directories are fresh folders in the system temp directory.
//!
//! Every run also has to be quiet: any app warning or error, and any simulator message that is not a known routine one, fails the
//! scenario unless the scenario says it expects it.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the scenarios run and what they use.
pub struct Ctx {
    pub sim: PathBuf,
    pub bucket: PathBuf,
    pub fixtures: PathBuf,
    /// Scratch space (fresh per invocation).
    pub tmp: PathBuf,
    /// Screenshots and captures, `target/bucket-e2e`.
    pub out: PathBuf,
    pub verbose: bool,
}

/// One simulator run.
#[derive(Default)]
struct Job {
    name: String,
    script: String,
    args: Vec<String>,
    files: Vec<PathBuf>,
    /// The app's data directory; fresh when `None`.
    data: Option<PathBuf>,
    /// Substrings of warning lines this run is expected to print.
    allow: Vec<&'static str>,
}

/// What a run left behind.
struct Run {
    stderr: String,
    code: i32,
    dir: PathBuf,
}

macro_rules! ensure {
    ($cond:expr, $($arg:tt)*) => {
        let ok: bool = $cond;
        if !ok {
            return Err(format!($($arg)*));
        }
    };
}

/// One report of `now_playing_playback`.
#[derive(Debug, Clone, Copy)]
struct Playback {
    state: u32,
    position_us: i64,
    rate: f32,
    flags: u32,
}

/// Routine simulator messages (everything else it prints is a finding).
const ROUTINE: &[&str] = &[
    "bucket-sim: Rusty Wave ",
    "bucket-sim: CHECKSUMS verified",
    "bucket-sim: wrote ",
    "bucket-sim: the app returned from bucket_main with status 0",
    // The worker pool parks in atomic waits and is abandoned at the end of the run, which the simulator documents.
    "bucket-sim: some threads are still blocked",
    "bucket-sim: script: ",
    // `bucket_save_state` returns 0 (nothing is kept in memory: the queue and the position are in the key-value store).
    "bucket-sim: hot reload: no state saved",
    "bucket-sim: SIGNATURE",
    "bucket-sim: unsigned",
];

impl Run {
    /// The app's trace lines, without the prefix.
    fn trace(&self) -> Vec<&str> {
        self.stderr.lines().filter_map(|l| l.strip_prefix("[app:trace] ")).collect()
    }

    fn trace_with(&self, prefix: &str) -> Vec<&str> {
        self.trace().into_iter().filter(|l| l.starts_with(prefix)).collect()
    }

    fn playbacks(&self) -> Vec<Playback> {
        self.trace_with("np.playback ")
            .into_iter()
            .filter_map(|l| {
                let f =
                    |key: &str| l.split_whitespace().find_map(|w| w.strip_prefix(key)).map(str::to_string);
                Some(Playback {
                    state: f("state=")?.parse().ok()?,
                    position_us: f("position_us=")?.parse().ok()?,
                    rate: f("rate=")?.parse().ok()?,
                    flags: f("flags=")?.parse().ok()?,
                })
            })
            .collect()
    }

    /// `key=value` of the last trace line starting with `prefix`.
    fn field(&self, prefix: &str, key: &str) -> Option<String> {
        let line = self.trace_with(prefix).into_iter().next_back()?;
        // The key starts a word, so `art=` is not found inside `artist=`.
        let at = line.find(&format!(" {key}="))? + key.len() + 2;
        let rest = &line[at..];
        if let Some(q) = rest.strip_prefix('"') {
            return q.split('"').next().map(str::to_string);
        }
        rest.split_whitespace().next().map(str::to_string)
    }

    fn shot(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Lines that should not be there.
    fn problems(&self, allow: &[&str]) -> Vec<String> {
        self.stderr
            .lines()
            .filter(|l| {
                !(l.starts_with("[app:trace]")
                    || l.starts_with("[app:info]")
                    || l.starts_with("[app:debug]")
                    || ROUTINE.iter().any(|r| l.starts_with(r))
                    || allow.iter().any(|a| l.contains(a)))
            })
            .map(str::to_string)
            .collect()
    }

    /// Frames and rate of the captured audio.
    fn wav(&self) -> Result<(u64, u32), String> {
        let path = self.dir.join("audio.wav");
        let b = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        ensure!(
            b.len() >= 44 && &b[0..4] == b"RIFF" && &b[8..12] == b"WAVE",
            "{} is not a WAV file",
            path.display()
        );
        let (mut at, mut rate, mut align) = (12, 0u32, 0u64);
        while at + 8 <= b.len() {
            let len = u32::from_le_bytes([b[at + 4], b[at + 5], b[at + 6], b[at + 7]]) as usize;
            match &b[at..at + 4] {
                b"fmt " => {
                    rate = u32::from_le_bytes([b[at + 12], b[at + 13], b[at + 14], b[at + 15]]);
                    align = u64::from(u16::from_le_bytes([b[at + 20], b[at + 21]]));
                }
                b"data" => {
                    let n = len.min(b.len() - at - 8) as u64;
                    ensure!(align > 0 && rate > 0, "the WAV has no fmt chunk before its data");
                    return Ok((n / align, rate));
                }
                _ => {}
            }
            at += 8 + len + (len & 1);
        }
        Err("the WAV has no data chunk".into())
    }

    /// Seconds of audio the first stream played.
    fn audio_secs(&self) -> Result<f64, String> {
        let (frames, rate) = self.wav()?;
        Ok(frames as f64 / f64::from(rate))
    }
}

/// Width and height of a PNG.
fn png_size(path: &Path) -> Result<(u32, u32), String> {
    let b = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    ensure!(b.len() > 24 && b[1..4] == *b"PNG", "{} is not a PNG", path.display());
    let rd = |at: usize| u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    Ok((rd(16), rd(20)))
}

/// A picture with something in it: the empty state alone is about 70 KB, a black frame a few hundred bytes.
fn png_has_content(path: &Path) -> Result<(), String> {
    let n = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
    ensure!(n > 20_000, "{} looks blank ({n} bytes)", path.display());
    Ok(())
}

fn same_file(a: &Path, b: &Path) -> Result<bool, String> {
    Ok(fs::read(a).map_err(|e| format!("{}: {e}", a.display()))?
        == fs::read(b).map_err(|e| format!("{}: {e}", b.display()))?)
}

impl Ctx {
    fn fixture(&self, rel: &str) -> Result<PathBuf, String> {
        let p = self.fixtures.join(rel);
        ensure!(p.exists(), "missing fixture {} (run `cargo xtask fixtures`)", p.display());
        Ok(p)
    }

    /// Run the simulator once.
    fn run(&self, job: &Job) -> Result<Run, String> {
        let dir = self.out.join(&job.name);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let data = match &job.data {
            Some(d) => d.clone(),
            None => {
                // Fresh and empty: a counter keeps two runs of one scenario apart.
                let d = self.tmp.join(format!(
                    "{}-{}",
                    job.name,
                    fs::read_dir(&self.tmp).map_or(0, Iterator::count)
                ));
                fs::create_dir_all(&d).map_err(|e| e.to_string())?;
                d
            }
        };
        let script_path = dir.join("script.txt");
        let script = job.script.replace("{out}", &dir.display().to_string());
        fs::write(&script_path, &script).map_err(|e| e.to_string())?;
        let mut cmd = Command::new(&self.sim);
        cmd.arg(&self.bucket)
            .args(["--headless", "--timeout", "120", "--log-level", "4", "--script"])
            .arg(&script_path)
            .arg("--audio-capture")
            .arg(dir.join("audio.wav"))
            .arg("--data-dir")
            .arg(&data)
            .args(&job.args)
            .args(&job.files);
        let out = cmd.output().map_err(|e| format!("{}: {e}", self.sim.display()))?;
        let run = Run {
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            code: out.status.code().unwrap_or(-1),
            dir,
        };
        if self.verbose {
            for l in run.stderr.lines() {
                println!("    | {l}");
            }
        }
        ensure!(run.code == 0, "the simulator exited with status {}:\n{}", run.code, tail(&run.stderr));
        let bad = run.problems(&job.allow);
        ensure!(bad.is_empty(), "unexpected output:\n  {}", bad.join("\n  "));
        Ok(run)
    }
}

fn tail(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    lines[lines.len().saturating_sub(12)..].join("\n")
}

fn within(what: &str, got: f64, lo: f64, hi: f64) -> Result<(), String> {
    ensure!((lo..=hi).contains(&got), "{what}: {got:.3} is outside {lo:.3}..{hi:.3}");
    Ok(())
}

fn sec(us: i64) -> f64 {
    us as f64 / 1e6
}

type Scenario = fn(&Ctx) -> Result<String, String>;

/// Every scenario, in the order they run.
const SCENARIOS: &[(&str, Scenario)] = &[
    ("empty", empty),
    ("play-video", play_video),
    ("play-baseline", play_baseline),
    ("play-flac", play_flac),
    ("pause-resume", pause_resume),
    ("seek", seek),
    ("transport", transport),
    ("queue-open", queue_open),
    ("library", library),
    ("restore", restore),
    ("hot-reload", hot_reload),
    ("slow-volume", slow_volume),
    ("slow-fail", slow_fail),
    ("audio-device", audio_device),
    ("window", window),
];

/// Run the scenarios whose name contains `only` (all when empty). Returns the number that failed.
pub fn run_all(ctx: &Ctx, only: &[String]) -> usize {
    let mut failed = 0;
    for (name, f) in SCENARIOS {
        if !only.is_empty() && !only.iter().any(|o| name.contains(o.as_str())) {
            continue;
        }
        let t = std::time::Instant::now();
        match f(ctx) {
            Ok(summary) => println!("ok   {name} ({:.1}s): {summary}", t.elapsed().as_secs_f64()),
            Err(e) => {
                failed += 1;
                println!("FAIL {name}: {e}");
            }
        }
    }
    failed
}

// ---- the scenarios ----

fn base(name: &str, script: &str) -> Job {
    Job { name: name.into(), script: script.into(), ..Job::default() }
}

/// The empty state.
fn empty(ctx: &Ctx) -> Result<String, String> {
    let mut j = base("empty", "1s screenshot {out}/empty.png\n1.5s terminate 2000\n");
    j.args = vec!["--size".into(), "1280x720".into()];
    let r = ctx.run(&j)?;
    ensure!(png_size(&r.shot("empty.png"))? == (1280, 720), "the canvas is not 1280x720");
    png_has_content(&r.shot("empty.png"))?;
    ensure!(r.trace_with("audio.open").is_empty(), "nothing was playing but audio was opened");
    ensure!(r.trace_with("np.meta").is_empty(), "an item was reported with nothing playing");
    ensure!(
        r.playbacks().iter().all(|p| p.state == 0),
        "now-playing says something plays: {:?}",
        r.playbacks()
    );
    Ok("empty state drawn at 1280x720, nothing playing".into())
}

/// Play a video (H.264 + AAC) to the end in the build the simulator picks (threads).
fn play_video(ctx: &Ctx) -> Result<String, String> {
    let mut j = base(
        "play-video",
        "1s screenshot {out}/t1.png\n3s screenshot {out}/t3.png\n7.5s screenshot {out}/end.png\n9s terminate 2000\n",
    );
    j.files = vec![ctx.fixture("h264_aac.mp4")?];
    j.args = vec!["--cpus".into(), "4".into()];
    let r = ctx.run(&j)?;
    // The threads build starts its pool on the threads the OS grants (the limit counts the main thread).
    let pool: u32 = r.field("threads:", "pool").and_then(|v| v.parse().ok()).unwrap_or(0);
    ensure!(pool >= 1, "the threads build started no pool: {:?}", r.trace_with("threads:"));
    ensure!(
        r.stderr.contains("build app.threads.wasm"),
        "the threads build was not picked:\n{}",
        tail(&r.stderr)
    );
    ensure!(r.field("audio.open", "got").as_deref() == Some("48000x2"), "audio was not opened at 48000x2");
    // The whole clip: 6 s of audio, played at 1x on the simulator's clock.
    let secs = r.audio_secs()?;
    within("audio played", secs, 5.95, 6.05)?;
    ensure!(
        r.field("np.meta", "title").is_some_and(|t| t.contains("h264_aac")),
        "title is not the file name"
    );
    ensure!(r.field("np.meta", "video").as_deref() == Some("true"), "the item is not reported as video");
    let pb = r.playbacks();
    ensure!(pb.iter().any(|p| p.state == 1), "never reported Playing: {pb:?}");
    // At the end the item stops at its full length.
    ensure!(
        pb.last().is_some_and(|p| p.state == 2 && (5_900_000..=6_100_000).contains(&p.position_us)),
        "the last report is not the end of the item: {:?}",
        pb.last()
    );
    png_has_content(&r.shot("t1.png"))?;
    ensure!(
        !same_file(&r.shot("t1.png"), &r.shot("t3.png"))?,
        "the picture did not change between 1 s and 3 s"
    );
    Ok(format!("{secs:.2} s of audio, {} now-playing reports", pb.len()))
}

/// The same with the baseline module (an engine without threads).
fn play_baseline(ctx: &Ctx) -> Result<String, String> {
    let mut j = base("play-baseline", "2s screenshot {out}/t2.png\n3s terminate 2000\n");
    j.args = vec!["--no-threads".into(), "--no-simd".into()];
    j.files = vec![ctx.fixture("h264_aac.mp4")?];
    let r = ctx.run(&j)?;
    ensure!(r.stderr.contains("build app.wasm"), "the baseline build was not picked:\n{}", tail(&r.stderr));
    ensure!(r.trace_with("threads:").is_empty(), "the baseline build must not start threads");
    let secs = r.audio_secs()?;
    within("audio played in 3 s", secs, 2.85, 3.0)?;
    ensure!(r.field("audio.exit", "underruns").as_deref() == Some("0"), "the audio underran");
    png_has_content(&r.shot("t2.png"))?;
    Ok(format!("baseline build plays at 1x ({secs:.2} s of audio in 3 s)"))
}

/// An audio-only FLAC: tags and cover in now-playing, the audio clock at 1x.
fn play_flac(ctx: &Ctx) -> Result<String, String> {
    let mut j = base("play-flac", "1s visualizer off\n1.5s visualizer on\n2s terminate 2000\n");
    j.files = vec![ctx.fixture("audio/tone.flac")?];
    let r = ctx.run(&j)?;
    ensure!(
        r.field("np.meta", "title").as_deref() == Some("Chirp Étude"),
        "title: {:?}",
        r.field("np.meta", "title")
    );
    ensure!(r.field("np.meta", "artist").as_deref() == Some("The Tones"), "artist");
    ensure!(r.field("np.meta", "album").as_deref() == Some("Pure"), "album");
    ensure!(r.field("np.meta", "video").as_deref() == Some("false"), "a FLAC is not video");
    let art: u64 = r.field("np.meta", "art").and_then(|a| a.parse().ok()).unwrap_or(0);
    ensure!(art > 100, "the cover was not sent ({art} bytes)");
    ensure!(
        r.field("np.meta", "duration_us")
            .and_then(|d| d.parse::<i64>().ok())
            .is_some_and(|d| (3_200_000..3_400_000).contains(&d)),
        "duration"
    );
    // 2 s of script time, started within a few ms: the stream played about 2 s.
    let secs = r.audio_secs()?;
    within("audio played in 2 s", secs, 1.9, 2.0)?;
    let frames: u64 = r.field("audio.exit", "frames_played").and_then(|v| v.parse().ok()).unwrap_or(0);
    let heard = frames as f64 / f64::from(r.wav()?.1);
    within("the audio clock's frames_played", heard, 1.85, 2.0)?;
    Ok(format!("tags and a {art} byte cover reported; {secs:.2} s played in 2 s, clock says {heard:.2} s"))
}

/// Pause and resume: the audio device must stop with the clock.
fn pause_resume(ctx: &Ctx) -> Result<String, String> {
    let mut j = base(
        "pause-resume",
        "1s key k\n1.5s screenshot {out}/paused-a.png\n1.9s screenshot {out}/paused-b.png\n2s key k\n4s terminate 2000\n",
    );
    j.files = vec![ctx.fixture("h264_aac.mp4")?];
    let r = ctx.run(&j)?;
    // Playing 0..1 s and 2..4 s: 3 s of audio. A player that leaves the device running while paused plays 4 s.
    let secs = r.audio_secs()?;
    within("audio played (1 s paused out of 4)", secs, 2.9, 3.05)?;
    let trace = r.trace_with("audio.paused");
    // The start of an item pauses and releases the device once; then the user's pause and resume.
    ensure!(trace.ends_with(&["audio.paused true", "audio.paused false"]), "the device was told {trace:?}");
    let states: Vec<u32> = r.playbacks().iter().map(|p| p.state).collect();
    ensure!(
        states.windows(3).any(|w| w == [1, 2, 1]),
        "now-playing did not go Playing, Paused, Playing: {states:?}"
    );
    ensure!(same_file(&r.shot("paused-a.png"), &r.shot("paused-b.png"))?, "the picture moved while paused");
    Ok(format!("{secs:.2} s played of 4 s with 1 s paused; the device and the picture stopped"))
}

/// Seeking by key and by click on the seek bar.
fn seek(ctx: &Ctx) -> Result<String, String> {
    let mut j = base(
        "seek",
        "1s key l\n2s screenshot {out}/plus10.png\n2.5s key j\n3s screenshot {out}/minus10.png\n\
         3.5s pointer_move 640 600\n3.6s click 640 646\n4.5s screenshot {out}/click.png\n5s terminate 2000\n",
    );
    j.files = vec![ctx.fixture("av1_opus_60s.webm")?];
    let r = ctx.run(&j)?;
    let pb = r.playbacks();
    let pos: Vec<f64> = pb.iter().map(|p| sec(p.position_us)).collect();
    // The key jumps are 10 s: playing at about 1 s, +10 s is about 11 s, then -10 s about 2 s, and the click on the middle of the
    // bar is about 30 s. Each jump is a report (a jump over 0.5 s from the extrapolation).
    ensure!(
        pos.iter().any(|p| (10.5..=11.8).contains(p)),
        "no report near 11 s after the +10 s seek: {pos:?}"
    );
    ensure!(
        pos.iter().any(|p| (1.5..=3.2).contains(p) && pos.len() > 2),
        "no report near 2 s after the -10 s seek: {pos:?}"
    );
    ensure!(
        pos.iter().any(|p| (28.0..=32.0).contains(p)),
        "no report near 30 s after the click on the bar: {pos:?}"
    );
    ensure!(
        !same_file(&r.shot("plus10.png"), &r.shot("minus10.png"))?,
        "the picture did not change with the seek"
    );
    Ok(format!("reports at {:?} s", pos.iter().map(|p| (p * 10.0).round() / 10.0).collect::<Vec<_>>()))
}

/// Now-playing and the shell's transport commands.
fn transport(ctx: &Ctx) -> Result<String, String> {
    let script = "\
1s transport pause\n\
1.5s transport play\n\
2s transport seek_to 4s\n\
2.5s transport seek_by -2s\n\
3s transport rate 1.5\n\
3.5s transport volume 0.5\n\
4s media_key play_pause\n\
4.5s media_key play_pause\n\
5s transport next\n\
5.5s transport stop\n\
6s screenshot {out}/stopped.png\n\
6.5s terminate 2000\n";
    let mut j = base("transport", script);
    j.files = vec![ctx.fixture("h264_aac.mp4")?];
    let r = ctx.run(&j)?;
    let cmds = r.trace_with("np.command ");
    let want = [
        "Pause",
        "Play",
        "SeekTo(4000000)",
        "SeekBy(-2000000)",
        "SetRate(1.5)",
        "SetVolume(0.5)",
        "Toggle",
        "Toggle",
        "Next",
        "Stop",
    ];
    ensure!(
        cmds.len() >= want.len() && want.iter().zip(&cmds).all(|(w, c)| c.ends_with(w)),
        "commands seen: {cmds:?}"
    );
    let pb = r.playbacks();
    let st: Vec<u32> = pb.iter().map(|p| p.state).collect();
    // Playing, Paused (command), Playing, ... Paused (media key), Playing, then Stop (Paused at the start).
    ensure!(st.contains(&1) && st.contains(&2) && st.last() == Some(&2), "states: {st:?}");
    ensure!(
        pb.iter().any(|p| p.state == 1 && (3.5..=4.6).contains(&sec(p.position_us))),
        "no report near 4 s after seek_to 4s: {pb:?}"
    );
    ensure!(pb.iter().any(|p| (p.rate - 1.5).abs() < 1e-6), "the rate 1.5 was not reported");
    ensure!(pb.iter().any(|p| p.flags & 0b100 != 0), "can_seek was never set");
    // Stopped goes back to the start.
    ensure!(pb.last().is_some_and(|p| p.position_us == 0), "Stop did not rewind: {:?}", pb.last());
    ensure!(r.field("audio.exit", "underruns").as_deref() == Some("0"), "the audio underran");
    // A paused or stopped player holds the audio: frames played stop growing, so the capture is well short of the run.
    let secs = r.audio_secs()?;
    ensure!(secs < 5.0, "audio kept playing through the pauses: {secs:.2} s");
    Ok(format!("{} commands, states {st:?}", cmds.len()))
}

/// Several files given at launch become one queue (one `OPEN` each, one group: flag bit 1 on all but the last).
fn queue_open(ctx: &Ctx) -> Result<String, String> {
    let mut j = base("queue-open", "1s key q\n1.5s screenshot {out}/queue.png\n2s terminate 2000\n");
    j.files =
        vec![ctx.fixture("audio/tone.flac")?, ctx.fixture("audio/tone.ogg")?, ctx.fixture("audio/cbr.mp3")?];
    let r = ctx.run(&j)?;
    ensure!(r.trace_with("open ").len() == 3, "three files opened, not: {:?}", r.trace_with("open "));
    // The queue shows all three: Next becomes available.
    let pb = r.playbacks();
    ensure!(pb.iter().any(|p| p.flags & 1 != 0), "can_next was never set with three files queued: {pb:?}");
    Ok("three launch files, one queue".into())
}

/// The folder library: add a folder, the OS walks it, the listing arrives, play a track, and the index survives a restart.
fn library(ctx: &Ctx) -> Result<String, String> {
    let dir = ctx.fixture("library/music")?;
    let expected = count_files(&dir);
    let data = ctx.tmp.join("library-data");
    fs::create_dir_all(&data).map_err(|e| e.to_string())?;
    let script = "\
0.5s key b\n\
0.8s click 690 349\n\
3s key 3\n\
4s screenshot {out}/tracks.png\n\
4.2s click 500 150\n\
4.3s click 500 150\n\
6s screenshot {out}/playing.png\n\
7s terminate 2000\n";
    let mut j = base("library", script);
    j.args = vec!["--pick".into(), dir.display().to_string()];
    j.data = Some(data.clone());
    // The fixture tracks are 0.4 to 0.6 s long, so the next item starts within 0.5 s of where the shell would have extrapolated
    // the last one and the Simulator reads the report that follows the new metadata as a needless repeat (see the review).
    let r = ctx.run(&j)?;
    ensure!(r.trace_with("library.folder_added").len() == 1, "FOLDER_ADDED: {:?}", r.trace_with("library."));
    let listings = r.trace_with("library.listing ");
    ensure!(listings.len() == 1, "expected exactly one walk (the OS starts it), got {listings:?}");
    let files: usize = r.field("library.listing ", "files").and_then(|f| f.parse().ok()).unwrap_or(0);
    ensure!(files == expected, "the listing has {files} files, the folder has {expected}");
    ensure!(r.field("np.meta", "title").is_some(), "double-clicking a track did not start it");
    ensure!(r.field("audio.open", "got").is_some(), "no audio stream for the track");
    png_has_content(&r.shot("tracks.png"))?;
    // A second launch with the same data: the OS sends the last listing at launch, before any rescan.
    let mut j2 =
        base("library-relaunch", "2s key 3\n3s screenshot {out}/relaunch.png\n3.5s terminate 2000\n");
    j2.data = Some(data);
    let r2 = ctx.run(&j2)?;
    let again: usize = r2.field("library.listing ", "files").and_then(|f| f.parse().ok()).unwrap_or(0);
    ensure!(again == expected, "the launch listing has {again} files, expected {expected}");
    Ok(format!("{expected} files listed once, a track played, the launch listing came back with {again}"))
}

fn count_files(dir: &Path) -> usize {
    let mut n = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                n += 1;
            }
        }
    }
    n
}

/// A restart: the queue comes back paused at the position it had (the file's id is stable across launches).
fn restore(ctx: &Ctx) -> Result<String, String> {
    let data = ctx.tmp.join("restore-data");
    fs::create_dir_all(&data).map_err(|e| e.to_string())?;
    // The app resumes only a position of at least 5 s and with at least 10 s left, so this needs a long file.
    let mut a = base("restore-1", "7s terminate 2000\n");
    a.files = vec![ctx.fixture("av1_opus_60s.webm")?];
    a.data = Some(data.clone());
    let r1 = ctx.run(&a)?;
    ensure!(r1.field("audio.open", "got").is_some(), "the first run did not play");
    let mut b = base("restore-2", "1s screenshot {out}/restored.png\n1.5s terminate 2000\n");
    b.data = Some(data);
    let r2 = ctx.run(&b)?;
    ensure!(
        r2.field("np.meta", "title").is_some_and(|t| t.contains("av1_opus")),
        "the item did not come back: {:?}",
        r2.trace()
    );
    let pb = r2.playbacks();
    let p = pb.last().ok_or("no now-playing report after the restart")?;
    ensure!(p.state == 2, "the restored item should be paused, state {}", p.state);
    within("restored position", sec(p.position_us), 6.8, 7.1)?;
    ensure!(r2.audio_secs()? == 0.0, "a paused restore played audio");
    png_has_content(&r2.shot("restored.png"))?;
    Ok(format!("paused at {:.2} s", sec(p.position_us)))
}

/// A hot reload while playing: `RELOAD` first, the state saved in the next `events_wait`, a fresh instance with fresh memory and
/// `launch_reason() = 1`. Nothing is kept in memory, so the new instance restores from the key-value store like a restart.
fn hot_reload(ctx: &Ctx) -> Result<String, String> {
    // Opened while running (an `OPEN` with the flag). The OS sends launch files only on a normal launch, so the new instance
    // (`launch_reason` 1) has to restore the queue on its own.
    let file = ctx.fixture("av1_opus_60s.webm")?;
    let script = format!(
        "0.5s open \"{}\"\n7.5s reload\n9.5s screenshot {{out}}/reloaded.png\n10s terminate 2000\n",
        file.display()
    );
    let r = ctx.run(&base("hot-reload", &script))?;
    ensure!(r.trace_with("open ").len() == 1, "the file was opened twice: {:?}", r.trace_with("open "));
    let metas = r.trace_with("np.meta ").len();
    ensure!(metas >= 3, "the new instance did not report the item again ({metas} reports)");
    let p = r.playbacks().last().copied().ok_or("no report after the reload")?;
    ensure!(p.state == 2, "the item should come back paused, state {}", p.state);
    within("position after the reload", sec(p.position_us), 6.8, 7.6)?;
    png_has_content(&r.shot("reloaded.png"))?;
    Ok(format!("restored paused at {:.2} s", sec(p.position_us)))
}

/// A cold volume: every first read, key and id is `-BUSY` until `IO_READY`.
fn slow_volume(ctx: &Ctx) -> Result<String, String> {
    let mut j = base("slow-volume", "1.5s screenshot {out}/t1.png\n5s terminate 2000\n");
    j.args = vec!["--slow-volume".into(), "--slow-volume-ms".into(), "30".into()];
    j.files = vec![ctx.fixture("h264_aac.mp4")?];
    let r = ctx.run(&j)?;
    let secs = r.audio_secs()?;
    within("audio played from a cold volume", secs, 4.8, 5.0)?;
    ensure!(
        r.field("audio.exit", "underruns").as_deref() == Some("0"),
        "the audio underran on a slow volume"
    );
    // The OS walks a cold folder and the library reads tags through ids and handles that are all cold: the walk finishes late
    // (on the simulator's clock) but completely.
    let dir = ctx.fixture("library/music")?;
    let script =
        "0.5s key b\n0.8s click 690 349\n60s key 3\n61s screenshot {out}/tracks.png\n62s terminate 2000\n";
    let mut l = base("slow-library", script);
    l.args = vec![
        "--slow-volume".into(),
        "--slow-volume-ms".into(),
        "20".into(),
        "--pick".into(),
        dir.display().to_string(),
    ];
    let r2 = ctx.run(&l)?;
    let files: usize = r2.field("library.listing ", "files").and_then(|f| f.parse().ok()).unwrap_or(0);
    ensure!(files == count_files(&dir), "the cold walk listed {files} files");
    png_has_content(&r2.shot("tracks.png"))?;
    Ok(format!("plays {secs:.2} s from a cold volume; a cold library of {files} files is listed"))
}

/// A fetch that fails: `IO_READY` arrives and the retry says `-IO`. The player must report it and go on, not hang or crash.
fn slow_fail(ctx: &Ctx) -> Result<String, String> {
    let mut j = base("slow-fail", "2s screenshot {out}/failed.png\n2.5s terminate 2000\n");
    j.args = vec!["--slow-volume".into(), "--slow-fail".into(), "h264_aac".into()];
    j.files = vec![ctx.fixture("h264_aac.mp4")?];
    j.allow = vec![];
    let r = ctx.run(&j)?;
    ensure!(r.trace_with("audio.open").is_empty(), "audio opened for a file that cannot be read");
    png_has_content(&r.shot("failed.png"))?;
    // Keys that cannot be loaded (the saved library): the player starts without them, says so in the log, and the OS's own launch
    // listing still fills the library.
    let dir = ctx.fixture("library/music")?;
    let data = ctx.tmp.join("slow-fail-data");
    fs::create_dir_all(&data).map_err(|e| e.to_string())?;
    let mut a = base("slow-fail-seed", "0.5s key b\n0.8s click 690 349\n4s terminate 2000\n");
    a.args = vec!["--pick".into(), dir.display().to_string()];
    a.data = Some(data.clone());
    ctx.run(&a)?;
    let mut k =
        base("slow-fail-kv", "0.5s key b\n8s key 3\n9s screenshot {out}/kv.png\n10s terminate 2000\n");
    k.args = vec!["--slow-volume".into(), "--slow-fail".into(), "library/index".into()];
    k.data = Some(data);
    k.allow = vec!["kv_load `library/index"];
    let r2 = ctx.run(&k)?;
    ensure!(
        r2.stderr.contains("kv_load `library/index` failed: IO"),
        "the failing key was never reported:\n{}",
        tail(&r2.stderr)
    );
    ensure!(r2.field("library.listing ", "files").is_some(), "the launch listing did not arrive");
    png_has_content(&r2.shot("kv.png"))?;
    Ok("an unreadable file and an unreadable saved library are survived".into())
}

/// The audio device changes and then fails: playback goes on silently and the user is told.
fn audio_device(ctx: &Ctx) -> Result<String, String> {
    let mut j = base(
        "audio-device",
        "1s audio_device 44100\n2s audio_error\n2.2s screenshot {out}/error.png\n4s screenshot {out}/later.png\n5s terminate 2000\n",
    );
    j.files = vec![ctx.fixture("h264_aac.mp4")?];
    j.allow = vec!["the audio stream failed"];
    let r = ctx.run(&j)?;
    ensure!(r.stderr.contains("the audio stream failed: NO_DEVICE"), "the failure was not noticed");
    ensure!(
        !same_file(&r.shot("error.png"), &r.shot("later.png"))?,
        "the picture stopped after the audio failed"
    );
    Ok("device change absorbed, failure survived with the picture going on".into())
}

/// The canvas: resize, scale, fullscreen and hidden windows (`canvas_present` is strict and says `-BUSY` for a stale size).
fn window(ctx: &Ctx) -> Result<String, String> {
    let script = "\
1s resize 800 600\n\
1.2s screenshot {out}/800x600.png\n\
1.5s resize 1600 900 2\n\
1.7s screenshot {out}/1600x900.png\n\
2s fullscreen on\n\
2.2s screenshot {out}/fullscreen.png\n\
2.5s fullscreen off\n\
3s visibility minimized\n\
3.5s visibility visible\n\
3.7s screenshot {out}/back.png\n\
3.8s resize 1000 500\n\
3.8s resize 640 360\n\
3.9s screenshot {out}/burst.png\n\
4.5s terminate 2000\n";
    let mut j = base("window", script);
    j.files = vec![ctx.fixture("h264_aac.mp4")?];
    let r = ctx.run(&j)?;
    for (name, size) in [
        ("800x600.png", (800, 600)),
        ("1600x900.png", (1600, 900)),
        ("fullscreen.png", (1920, 1080)),
        ("burst.png", (640, 360)),
    ] {
        ensure!(
            png_size(&r.shot(name))? == size,
            "{name}: canvas is {:?}, expected {size:?}",
            png_size(&r.shot(name))
        );
        png_has_content(&r.shot(name))?;
    }
    png_has_content(&r.shot("back.png"))?;
    Ok("800x600, 1600x900 at 2x, fullscreen, hidden and back, a burst of resizes: all drawn at the final size".into())
}
