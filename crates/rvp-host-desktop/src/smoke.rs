//! What `--screenshot`, `--report` and `--exit-after` do: a scripted run for tests and for checking an installed package.
use crate::cli::Options;
use crate::host::DesktopHost;
use rvp_app::App;
use rvp_host::{InputEvent, Key, Modifiers};
use rvp_ui::{MediaState, Mode};
use std::time::Instant;

/// Tracks a scripted run.
pub struct Smoke {
    start: Instant,
    exit_after: Option<f64>,
    shot: Option<std::path::PathBuf>,
    shot_after: Option<f64>,
    shot_done: bool,
    report: Option<std::path::PathBuf>,
    /// First and latest `(wall seconds, position us)` while playing from one second in.
    first: Option<(f64, i64)>,
    last: Option<(f64, i64)>,
    saw_playing: bool,
    saw_ended: bool,
    max_position_us: i64,
    video_size: (u32, u32),
    /// Keys still to press, earliest first.
    presses: Vec<(f64, Key, Modifiers)>,
}

/// A key as written on the command line (`b`, `ctrl+f`, `Space`, `Left`).
pub fn parse_key(text: &str) -> Option<(Key, Modifiers)> {
    let mut mods = Modifiers::default();
    let mut rest = text;
    loop {
        let lower = rest.to_ascii_lowercase();
        if let Some(r) = lower.strip_prefix("ctrl+") {
            mods.ctrl = true;
            rest = &rest[rest.len() - r.len()..];
        } else if let Some(r) = lower.strip_prefix("shift+") {
            mods.shift = true;
            rest = &rest[rest.len() - r.len()..];
        } else if let Some(r) = lower.strip_prefix("alt+") {
            mods.alt = true;
            rest = &rest[rest.len() - r.len()..];
        } else {
            break;
        }
    }
    let key = match rest.to_ascii_lowercase().as_str() {
        "space" => Key::Space,
        "enter" | "return" => Key::Enter,
        "escape" | "esc" => Key::Escape,
        "left" => Key::Left,
        "right" => Key::Right,
        "up" => Key::Up,
        "down" => Key::Down,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::Other("PageUp".into()),
        "pagedown" => Key::Other("PageDown".into()),
        "delete" => Key::Other("Delete".into()),
        "backspace" => Key::Other("Backspace".into()),
        "tab" => Key::Other("Tab".into()),
        _ => {
            let mut it = rest.chars();
            match (it.next(), it.next()) {
                (Some(c), None) => Key::Char(c),
                _ => return None,
            }
        }
    };
    Some((key, mods))
}

impl Smoke {
    /// Active only if the command line asked for something scripted.
    pub fn new(o: &Options) -> Option<Self> {
        if o.exit_after.is_none() && o.screenshot.is_none() && o.report.is_none() && o.press.is_empty() {
            return None;
        }
        let mut presses: Vec<(f64, Key, Modifiers)> =
            o.press.iter().filter_map(|(t, k)| parse_key(k).map(|(key, mods)| (*t, key, mods))).collect();
        presses.sort_by(|a, b| a.0.total_cmp(&b.0));
        Some(Self {
            start: Instant::now(),
            exit_after: o.exit_after,
            shot: o.screenshot.clone(),
            shot_after: o.screenshot_after,
            shot_done: false,
            report: o.report.clone(),
            first: None,
            last: None,
            saw_playing: false,
            saw_ended: false,
            max_position_us: 0,
            video_size: (0, 0),
            presses,
        })
    }

    /// Key events that are due now (a press and its release).
    pub fn due_input(&mut self) -> Vec<InputEvent> {
        let t = self.start.elapsed().as_secs_f64();
        let mut out = Vec::new();
        while self.presses.first().is_some_and(|p| p.0 <= t) {
            let (_, key, mods) = self.presses.remove(0);
            out.push(InputEvent::KeyDown { key: key.clone(), mods, repeat: false });
            out.push(InputEvent::KeyUp { key, mods });
        }
        out
    }

    /// True when the window should keep a copy of what it presents.
    pub fn wants_frames(&self) -> bool {
        self.shot.is_some()
    }

    /// Look at the app after a tick. Returns true when the run is over.
    pub fn tick(&mut self, app: &App, host: &mut DesktopHost) -> bool {
        let t = self.start.elapsed().as_secs_f64();
        let m = app.model();
        if m.state == MediaState::Playing {
            self.saw_playing = true;
            if m.position_us >= 1_000_000 {
                if self.first.is_none() {
                    self.first = Some((t, m.position_us));
                }
                self.last = Some((t, m.position_us));
            }
        }
        if m.state == MediaState::Ended {
            self.saw_ended = true;
        }
        self.max_position_us = self.max_position_us.max(m.position_us);
        if host.video.width > 0 {
            self.video_size = (host.video.width, host.video.height);
        }
        if let (Some(after), false) = (self.shot_after, self.shot_done)
            && t >= after
        {
            self.take_screenshot(host);
        }
        // Without an exit time, a run ends when the media has played out and everything asked for has been written.
        self.exit_after.is_some_and(|s| t >= s)
    }

    fn take_screenshot(&mut self, host: &DesktopHost) {
        self.shot_done = true;
        let Some(path) = &self.shot else { return };
        let (w, h, _) = host.surface.size;
        if host.surface.last.len() != w as usize * h as usize * 4 {
            eprintln!("rusty-wave: no picture to save yet");
            return;
        }
        if let Err(e) = write_png(path, w, h, &host.surface.last) {
            eprintln!("rusty-wave: could not write {}: {e}", path.display());
        }
    }

    /// The run is over: write the screenshot (if not yet) and the report.
    pub fn finish(&mut self, app: &App, host: &DesktopHost) {
        if !self.shot_done {
            self.take_screenshot(host);
        }
        let Some(path) = &self.report else { return };
        let m = app.model();
        let ratio = match (self.first, self.last) {
            (Some((t0, p0)), Some((t1, p1))) if t1 - t0 >= 1.0 => {
                format!("{:.4}", (p1 - p0) as f64 / 1e6 / (t1 - t0))
            }
            _ => "null".into(),
        };
        let span = match (self.first, self.last) {
            (Some((t0, _)), Some((t1, _))) => t1 - t0,
            _ => 0.0,
        };
        let state = format!("{:?}", m.state);
        let json = format!(
            "{{\n  \"version\": \"{}\",\n  \"state\": \"{}\",\n  \"saw_playing\": {},\n  \"saw_ended\": {},\n  \"position_ms\": {},\n  \"max_position_ms\": {},\n  \"duration_ms\": {},\n  \"clock_ratio\": {},\n  \"clock_span_s\": {:.3},\n  \"title\": {},\n  \"artist\": {},\n  \"queue_len\": {},\n  \"video_frames\": {},\n  \"video_size\": [{}, {}],\n  \"frames_presented\": {},\n  \"window\": [{}, {}, {:.2}],\n  \"audio_backend\": {},\n  \"audio_frames_written\": {},\n  \"audio_frames_played\": {},\n  \"audio_failed\": {},\n  \"media_controls\": {},\n  \"mode\": \"{}\",\n  \"fullscreen\": {},\n  \"library_tracks\": {},\n  \"library_albums\": {},\n  \"library_videos\": {},\n  \"library_playable_tracks\": {},\n  \"library_playable_videos\": {},\n  \"library_roots_connected\": {},\n  \"library_roots\": [{}],\n  \"dialog\": {},\n  \"first_run\": {},\n  \"crossfade\": {},\n  \"crossfade_secs\": {},\n  \"auto_level\": {},\n  \"target_lufs\": {},\n  \"level_mode\": \"{}\"\n}}\n",
            env!("CARGO_PKG_VERSION"),
            state,
            self.saw_playing,
            self.saw_ended,
            m.position_us / 1000,
            self.max_position_us / 1000,
            m.duration_us.map_or(-1, |d| d / 1000),
            ratio,
            span,
            json_str(&m.title),
            json_str(&m.artist),
            app.playlist().len(),
            host.video.count,
            self.video_size.0,
            self.video_size.1,
            host.surface.presents,
            host.surface.size.0,
            host.surface.size.1,
            host.surface.size.2,
            json_str(&host.audio.backend()),
            host.audio.frames_written(),
            host.audio.frames_played(),
            host.audio.failed(),
            host.media.is_some(),
            if app.ui().mode() == Mode::Library { "library" } else { "player" },
            m.fullscreen,
            app.library().track_count(),
            app.library().albums().len(),
            app.library().video_count(),
            app.library().all_tracks().iter().filter(|t| !t.src.is_empty()).count(),
            app.library().all_videos().iter().filter(|v| !v.src.is_empty()).count(),
            app.library().roots().iter().filter(|r| r.connected).count(),
            app.library().roots().iter().map(|r| json_str(&r.name)).collect::<Vec<_>>().join(", "),
            m.dialog.as_ref().map_or("null".to_string(), |d| json_str(&d.title)),
            app.setup().is_first_run(),
            app.audio_settings().crossfade,
            app.audio_settings().crossfade_secs,
            app.audio_settings().auto_level,
            app.audio_settings().target_lufs,
            if app.audio_settings().level_mode == rvp_core::LevelMode::Album { "album" } else { "track" },
        );
        if let Err(e) = std::fs::write(path, json) {
            eprintln!("rusty-wave: could not write {}: {e}", path.display());
        }
    }
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Write an RGBA picture as a PNG.
pub fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let mut wr = enc.write_header().map_err(|e| e.to_string())?;
    wr.write_image_data(rgba).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_strings_are_escaped() {
        assert_eq!(json_str("a\"b\\c\n"), "\"a\\\"b\\\\c\\u000a\"");
    }

    #[test]
    fn keys_parse() {
        assert_eq!(parse_key("b"), Some((Key::Char('b'), Modifiers::default())));
        let (k, m) = parse_key("Ctrl+F").unwrap();
        assert_eq!((k, m.ctrl), (Key::Char('F'), true));
        assert_eq!(parse_key("space").unwrap().0, Key::Space);
        assert!(parse_key("shift+Left").unwrap().1.shift);
        assert_eq!(parse_key("nope"), None);
    }

    #[test]
    fn png_round_trips_through_the_decoder() {
        let path = std::env::temp_dir().join(format!("rvp-smoke-{}.png", std::process::id()));
        let rgba: Vec<u8> = (0..4 * 3 * 4).map(|i| i as u8).collect();
        write_png(&path, 4, 3, &rgba).unwrap();
        let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()));
        let mut r = dec.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let info = r.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (4, 3));
        assert_eq!(&buf[..rgba.len()], &rgba[..]);
        std::fs::remove_file(path).ok();
    }
}
