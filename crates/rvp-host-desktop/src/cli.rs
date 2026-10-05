//! The command line.
use std::path::PathBuf;

/// What `rusty-wave --help` says.
pub const HELP: &str = "\
Rusty Wave: a standalone media player for video and music.

USAGE:
    rusty-wave [OPTIONS] [FILE|FOLDER|file:// URI]...

Files open as the queue and the first one plays (subtitle files join the video that is playing, playlist files are imported).
A folder is added to the music library and scanned.

OPTIONS:
    -h, --help                 print this help
    -V, --version              print the version
        --fullscreen           start in full screen
        --window <W>x<H>       initial window size in logical pixels (default 1280x720)
        --no-audio             do not open an audio device (video plays, the clock runs, no sound)
        --no-media-keys        do not register with MPRIS or the system media controls
        --data-dir <DIR>       keep settings, library and queue in DIR (default: the user's data directory)

FOR SCRIPTS AND TESTS:
        --exit-after <SEC>     quit after SEC seconds
        --screenshot <PNG>     write the window's picture to PNG (at --screenshot-after, else at exit)
        --screenshot-after <SEC>
        --report <JSON>        write a short report (state, position, frames, audio clock rate) at exit
        --paused               open the first file paused
        --press <SEC>:<KEY>    press a key SEC seconds in (repeatable): a character, or Space, Enter, Escape, Left, Right, Up,
                               Down, Home, End; prefix with ctrl+ or shift+ (for example --press 2:b --press 3.5:ctrl+f)

KEYS:  Space play/pause, F or F11 full screen, B library/player, O open, arrows seek, Up/Down volume, M mute,
       [ ] speed, S A subtitle/audio track, N P next/previous, Q queue, Ctrl+F search. Right-click for the menu.
";

/// Parsed command line.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Options {
    /// Files, folders and URIs to open.
    pub inputs: Vec<String>,
    /// Print the help and exit.
    pub help: bool,
    /// Print the version and exit.
    pub version: bool,
    /// Start in full screen.
    pub fullscreen: bool,
    /// Initial logical size.
    pub size: Option<(u32, u32)>,
    /// No audio device.
    pub no_audio: bool,
    /// No MPRIS / SMTC.
    pub no_media_keys: bool,
    /// Where to keep data.
    pub data_dir: Option<PathBuf>,
    /// Quit after this many seconds.
    pub exit_after: Option<f64>,
    /// Write a screenshot here.
    pub screenshot: Option<PathBuf>,
    /// Seconds into the run to take it.
    pub screenshot_after: Option<f64>,
    /// Write a report here.
    pub report: Option<PathBuf>,
    /// Open the first file paused.
    pub paused: bool,
    /// Keys to press: `(seconds into the run, key text)`.
    pub press: Vec<(f64, String)>,
}

/// Parse `args` (without the program name).
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Options, String> {
    let mut o = Options::default();
    let mut it = args.into_iter();
    let mut only_files = false;
    while let Some(a) = it.next() {
        if only_files || !a.starts_with('-') || a == "-" {
            o.inputs.push(a);
            continue;
        }
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n.to_string(), Some(v.to_string())),
            _ => (a.clone(), None),
        };
        let mut value = |what: &str| -> Result<String, String> {
            inline.clone().or_else(|| it.next()).ok_or_else(|| format!("{name} needs {what}"))
        };
        match name.as_str() {
            "--" => only_files = true,
            "-h" | "--help" => o.help = true,
            "-V" | "--version" => o.version = true,
            "--fullscreen" => o.fullscreen = true,
            "--no-audio" => o.no_audio = true,
            "--no-media-keys" => o.no_media_keys = true,
            "--paused" => o.paused = true,
            "--window" => {
                let v = value("WxH")?;
                let (w, h) = v.split_once(['x', 'X']).ok_or("--window needs WxH, for example 1280x720")?;
                let (w, h): (u32, u32) = (
                    w.parse().map_err(|_| "--window: bad width")?,
                    h.parse().map_err(|_| "--window: bad height")?,
                );
                if !(160..=16384).contains(&w) || !(90..=16384).contains(&h) {
                    return Err("--window: size out of range".into());
                }
                o.size = Some((w, h));
            }
            "--data-dir" => o.data_dir = Some(PathBuf::from(value("a directory")?)),
            "--exit-after" => {
                o.exit_after = Some(value("seconds")?.parse().map_err(|_| "--exit-after: not a number")?)
            }
            "--screenshot" => o.screenshot = Some(PathBuf::from(value("a file name")?)),
            "--screenshot-after" => {
                o.screenshot_after =
                    Some(value("seconds")?.parse().map_err(|_| "--screenshot-after: not a number")?)
            }
            "--report" => o.report = Some(PathBuf::from(value("a file name")?)),
            "--press" => {
                let v = value("SEC:KEY")?;
                let (t, k) = v.split_once(':').ok_or("--press needs SEC:KEY, for example 2:b")?;
                o.press.push((t.parse().map_err(|_| "--press: bad time")?, k.to_string()));
            }
            other => return Err(format!("unknown option {other} (try --help)")),
        }
    }
    Ok(o)
}

/// A `file://` URI as a path (percent-decoded); anything else is returned as is.
pub fn input_to_path(s: &str) -> String {
    let Some(rest) = s.strip_prefix("file://") else { return s.to_string() };
    // `file://host/path` and `file:///path` both end in the path.
    let path = if rest.starts_with('/') { rest } else { rest.find('/').map_or(rest, |i| &rest[i..]) };
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let p = String::from_utf8_lossy(&out).into_owned();
    // Windows: `/C:/Music/a.mp3` -> `C:/Music/a.mp3`.
    if cfg!(windows) && p.len() > 2 && p.as_bytes()[0] == b'/' && p.as_bytes()[2] == b':' {
        p[1..].to_string()
    } else {
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Options, String> {
        parse(a.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_options_and_inputs() {
        let o = p(&[
            "--fullscreen",
            "a.mkv",
            "--window",
            "800x450",
            "--data-dir=/tmp/x",
            "b.mp3",
            "--exit-after",
            "2.5",
        ])
        .unwrap();
        assert_eq!(o.inputs, ["a.mkv", "b.mp3"]);
        assert!(o.fullscreen);
        assert_eq!(o.size, Some((800, 450)));
        assert_eq!(o.data_dir, Some(PathBuf::from("/tmp/x")));
        assert_eq!(o.exit_after, Some(2.5));
        assert!(p(&["--nope"]).is_err());
        assert!(p(&["--window", "big"]).is_err());
        assert!(p(&["--screenshot"]).is_err());
        assert_eq!(p(&["--", "-odd.mkv"]).unwrap().inputs, ["-odd.mkv"]);
        assert!(p(&["-h"]).unwrap().help && p(&["--version"]).unwrap().version);
    }

    #[test]
    fn file_uris_become_paths() {
        assert_eq!(input_to_path("file:///home/a/My%20Song%20%231.mp3"), "/home/a/My Song #1.mp3");
        assert_eq!(input_to_path("/plain/path.mp3"), "/plain/path.mp3");
        assert_eq!(input_to_path("file://localhost/x/y.flac"), "/x/y.flac");
        assert_eq!(input_to_path("file:///caf%C3%A9.mp3"), "/café.mp3");
        assert_eq!(input_to_path("file:///100%"), "/100%");
    }
}
