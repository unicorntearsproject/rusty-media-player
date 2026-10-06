//! The shell's media interface: now-playing (with `TRANSPORT` commands coming back) and the visualizer feed.
use crate::api;
use crate::shared::Limits;
use bucket_v0_sys::{self as sys, err, playback_flags, transport};
use rvp_host::{
    NowPlaying, NowPlayingMeta, PlayState, Playback, TransportCommand, VisualizerTap, VizBlock, VizSummary,
};
use std::collections::VecDeque;

/// Cut `s` to at most `max` bytes at a character boundary.
pub fn truncate_utf8(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// True when `data` starts like a PNG or a JPEG, which is all the OS accepts as cover art (`-INVALID` for anything else).
pub fn has_image_magic(data: &[u8]) -> bool {
    data.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
        || data.starts_with(&[0xFF, 0xD8, 0xFF])
}

/// `NowPlaying` over `now_playing_metadata`, `now_playing_playback` and `TRANSPORT` events.
pub struct RbNowPlaying {
    limits: Limits,
    commands: VecDeque<TransportCommand>,
    last_meta: Option<NowPlayingMeta>,
    last_playback: Option<Playback>,
    /// Covers dropped because they were too large or not PNG/JPEG.
    pub art_dropped: u32,
}

impl RbNowPlaying {
    /// A new one.
    pub fn new(limits: Limits) -> Self {
        Self { limits, commands: VecDeque::new(), last_meta: None, last_playback: None, art_dropped: 0 }
    }

    /// A transport command from the shell.
    pub fn push_command(&mut self, c: TransportCommand) {
        self.commands.push_back(c);
    }

    /// Send what was last reported again (the shell started showing now-playing after the item began).
    pub fn replay(&mut self) {
        if let Some(m) = self.last_meta.clone() {
            self.send_meta(&m);
        }
        if let Some(p) = self.last_playback {
            self.send_playback(&p);
        }
    }

    /// Give up the active session (`now_playing_clear`): the player quit or was stopped for good.
    pub fn clear(&mut self) {
        // SAFETY: no pointers.
        unsafe { sys::now_playing_clear() };
        self.last_meta = None;
        self.last_playback = None;
    }

    fn send_meta(&mut self, meta: &NowPlayingMeta) {
        let s = self.limits.string;
        let (title, artist, album) =
            (truncate_utf8(&meta.title, s), truncate_utf8(&meta.artist, s), truncate_utf8(&meta.album, s));
        // Covers the shell cannot take are left out instead of failing the whole report: only PNG and JPEG, within the limit.
        let art = meta.art.as_ref().filter(|a| {
            let ok = matches!(a.mime.as_str(), "image/png" | "image/jpeg")
                && has_image_magic(&a.data)
                && !a.data.is_empty()
                && a.data.len() <= self.limits.art;
            if !ok {
                self.art_dropped += 1;
            }
            ok
        });
        let (mime, bytes): (&str, &[u8]) = art.map_or(("", &[]), |a| (a.mime.as_str(), a.data.as_slice()));
        api::trace(&format!(
            "np.meta title={title:?} artist={artist:?} album={album:?} duration_us={} video={} art={}",
            meta.duration_us.unwrap_or(-1),
            meta.has_video,
            bytes.len()
        ));
        let raw = sys::NowPlayingMetaRaw {
            struct_size: 64,
            flags: 0,
            title_ptr: sys::ptr32(title.as_ptr()),
            title_len: title.len() as u32,
            artist_ptr: sys::ptr32(artist.as_ptr()),
            artist_len: artist.len() as u32,
            album_ptr: sys::ptr32(album.as_ptr()),
            album_len: album.len() as u32,
            art_mime_ptr: sys::ptr32(mime.as_ptr()),
            art_mime_len: mime.len() as u32,
            art_ptr: sys::ptr32(bytes.as_ptr()),
            art_len: bytes.len() as u32,
            duration_us: meta.duration_us.unwrap_or(-1),
            has_video: meta.has_video as u32,
            reserved: 0,
        };
        // SAFETY: `raw` is the 64-byte struct and the strings it points to outlive the call.
        let r = unsafe { sys::now_playing_metadata((&raw as *const sys::NowPlayingMetaRaw).cast()) };
        if (r == err::TOO_LARGE || r == err::INVALID) && !bytes.is_empty() {
            // The shell's limit is lower than we thought, or it does not take this picture: send it without the cover.
            self.art_dropped += 1;
            let bare = NowPlayingMeta { art: None, ..meta.clone() };
            self.send_meta(&bare);
        } else if r < 0 && r != err::UNSUPPORTED {
            api::warn(&format!("now_playing_metadata failed: {}", api::code_name(r)));
        }
    }

    fn send_playback(&mut self, p: &Playback) {
        let state = match p.state {
            PlayState::Stopped => sys::play_state::STOPPED,
            PlayState::Playing => sys::play_state::PLAYING,
            PlayState::Paused => sys::play_state::PAUSED,
        };
        let mut flags = 0;
        for (on, bit) in [
            (p.can_next, playback_flags::CAN_NEXT),
            (p.can_prev, playback_flags::CAN_PREV),
            (p.can_seek, playback_flags::CAN_SEEK),
        ] {
            if on {
                flags |= bit;
            }
        }
        // The OS refuses a negative position and a rate that is not a number (`-INVALID`): say what is meant instead.
        let rate = if p.rate.is_finite() { p.rate } else { 1.0 };
        let position_us = p.position_us.max(0);
        api::trace(&format!("np.playback state={state} position_us={position_us} rate={rate} flags={flags}"));
        let raw = sys::NowPlayingPlaybackRaw {
            struct_size: 40,
            state,
            rate,
            reserved0: 0,
            position_us,
            host_time_us: api::now_us(),
            flags,
            reserved1: 0,
        };
        // SAFETY: `raw` is the 40-byte struct.
        let r = unsafe { sys::now_playing_playback((&raw as *const sys::NowPlayingPlaybackRaw).cast()) };
        if r < 0 && r != err::UNSUPPORTED {
            api::warn(&format!("now_playing_playback failed: {}", api::code_name(r)));
        }
    }
}

impl NowPlaying for RbNowPlaying {
    fn set_metadata(&mut self, meta: &NowPlayingMeta) {
        self.last_meta = Some(meta.clone());
        self.send_meta(meta);
    }

    fn set_playback(&mut self, playback: &Playback) {
        self.last_playback = Some(*playback);
        self.send_playback(playback);
    }

    fn poll_command(&mut self) -> Option<TransportCommand> {
        self.commands.pop_front()
    }
}

/// A `TRANSPORT` event as the player's command (`None` for a command this version does not know).
pub fn transport_command(command: u32, value_us: i64, value_f32: f32) -> Option<TransportCommand> {
    Some(match command {
        transport::PLAY => TransportCommand::Play,
        transport::PAUSE => TransportCommand::Pause,
        transport::TOGGLE => TransportCommand::Toggle,
        transport::STOP => TransportCommand::Stop,
        transport::NEXT => TransportCommand::Next,
        transport::PREV => TransportCommand::Prev,
        transport::SEEK_TO => TransportCommand::SeekTo(value_us),
        transport::SEEK_BY => TransportCommand::SeekBy(value_us),
        transport::SET_RATE if value_f32.is_finite() && value_f32 > 0.0 => {
            TransportCommand::SetRate(value_f32)
        }
        transport::SET_VOLUME if value_f32.is_finite() => {
            TransportCommand::SetVolume(value_f32.clamp(0.0, 1.0))
        }
        _ => return None,
    })
}

/// One summary in the API's layout.
pub fn summary_raw(s: &VizSummary) -> sys::VizSummaryRaw {
    sys::VizSummaryRaw {
        struct_size: 176,
        onset: s.onset as u32,
        pts_us: s.pts_us,
        level: s.level,
        peak: s.peak,
        bands: s.bands,
        bass: s.bass,
        mid: s.mid,
        treble: s.treble,
        onset_strength: s.onset_strength,
        tempo_bpm: s.tempo_bpm,
        reserved: 0,
    }
}

/// Most summaries held before they are sent (a tick makes one to three).
const BATCH_MAX: usize = 64;

/// The visualizer tap: PCM goes to the shell as it is heard, and the summaries of one tick are sent together (`viz_summary_n`).
pub struct RbViz {
    pending: Vec<sys::VizSummaryRaw>,
    batching: bool,
    /// Calls to `viz_block` so far.
    pub blocks: u64,
    /// Summaries sent so far.
    pub summaries: u64,
}

impl RbViz {
    /// A tap with nothing pending.
    pub fn new() -> Self {
        Self { pending: Vec::new(), batching: true, blocks: 0, summaries: 0 }
    }

    /// Send the summaries collected since the last call (the driver calls this once per tick).
    pub fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let n = self.pending.len();
        let mut sent = false;
        if self.batching {
            // SAFETY: `pending` holds `n` summaries back to back; the stride is the first one's `struct_size`.
            let r = unsafe { sys::viz_summary_n(self.pending.as_ptr().cast(), n as i32) };
            if r == err::UNSUPPORTED {
                self.batching = false;
            } else {
                sent = true;
            }
        }
        if !sent {
            for s in &self.pending {
                // SAFETY: one 176-byte summary.
                unsafe { sys::viz_summary((s as *const sys::VizSummaryRaw).cast()) };
            }
        }
        self.summaries += n as u64;
        self.pending.clear();
    }
}

impl Default for RbViz {
    fn default() -> Self {
        Self::new()
    }
}

impl VisualizerTap for RbViz {
    fn push_block(&mut self, block: &VizBlock<'_>) {
        let ch = usize::from(block.channels).max(1);
        let frames = block.samples.len() / ch;
        if frames == 0 {
            return;
        }
        self.blocks += 1;
        // SAFETY: the buffer holds `frames` frames of `channels` samples.
        unsafe {
            sys::viz_block(
                block.pts_us,
                block.sample_rate as i32,
                i32::from(block.channels),
                block.samples.as_ptr(),
                api::len32(frames),
            )
        };
    }

    fn push_summary(&mut self, summary: &VizSummary) {
        self.pending.push(summary_raw(summary));
        if self.pending.len() >= BATCH_MAX {
            self.flush();
        }
    }
}
