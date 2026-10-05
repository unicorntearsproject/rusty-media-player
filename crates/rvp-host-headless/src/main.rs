//! `rvp-headless`: run the player without a display or sound card, in virtual time.
//!
//! ```text
//! rvp-headless play <file> [--audio-wav out.wav] [--seek AT:TO ...] [--rate R]   (times in seconds of stream time)
//! ```
use rvp_host_headless::{PlayOptions, play_file, write_wav_f32};
use std::process::ExitCode;

fn secs(s: &str) -> Option<i64> {
    s.parse::<f64>().ok().map(|v| (v * 1e6) as i64)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("play") || args.len() < 2 {
        eprintln!(
            "rvp-headless {}\nusage: rvp-headless play <file> [--audio-wav out.wav] [--seek AT:TO ...] [--rate R]",
            rvp_host_headless::VERSION
        );
        return ExitCode::from(2);
    }
    let file = &args[1];
    let mut wav = None;
    let mut opts = PlayOptions::default();
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--audio-wav" => {
                wav = args.get(i + 1).cloned();
                i += 2;
            }
            "--seek" => {
                let pair = args
                    .get(i + 1)
                    .and_then(|v| v.split_once(':'))
                    .and_then(|(a, b)| Some((secs(a)?, secs(b)?)));
                match pair {
                    Some(p) => opts.seeks.push(p),
                    None => {
                        eprintln!("--seek needs AT:TO in seconds");
                        return ExitCode::from(2);
                    }
                }
                i += 2;
            }
            "--rate" => {
                match args.get(i + 1).and_then(|v| v.parse::<f64>().ok()) {
                    Some(r) => opts.rate = Some(r),
                    None => {
                        eprintln!("--rate needs a number (0.25 to 4)");
                        return ExitCode::from(2);
                    }
                }
                i += 2;
            }
            other => {
                eprintln!("unknown argument `{other}`");
                return ExitCode::from(2);
            }
        }
    }
    let report = match play_file(file, &opts) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "state={:?} virtual_time={:.3}s duration={} audio_frames={} video_frames={} dropped={} max_drift_ms={:.1}",
        report.state,
        report.virtual_us as f64 / 1e6,
        report.duration_us.map_or("?".into(), |d| format!("{:.3}s", d as f64 / 1e6)),
        report.audio.len() / 2,
        report.video_stats.presented,
        report.video_stats.dropped,
        report.video_stats.max_drift_us as f64 / 1000.0
    );
    for w in &report.warnings {
        eprintln!("warning: {w}");
    }
    if let Some(e) = &report.error {
        eprintln!("error: {e}");
    }
    if let Some(w) = wav {
        if let Err(e) = write_wav_f32(w.as_ref(), &report.audio, 48_000, 2) {
            eprintln!("cannot write {w}: {e}");
            return ExitCode::FAILURE;
        }
    }
    if report.error.is_some() { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}
