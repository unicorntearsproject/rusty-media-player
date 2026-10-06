//! Host-neutral media interfaces: what is playing (the now-playing model, MPRIS-like) and the visualizer tap.
//!
//! These are the source of truth for every host: a browser maps [`NowPlaying`] to the Media Session API, a desktop
//! to MPRIS, Rusty Bucket to its shell's media interface. Nothing here depends on any one of them.
use alloc::string::String;
use alloc::vec::Vec;
use rvp_core::Timestamp;

/// A cover picture: encoded JPEG or PNG bytes.
pub use rvp_core::Art;

/// What is playing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NowPlayingMeta {
    /// Title (the tag, or the file name without its extension).
    pub title: String,
    /// Artist, empty if unknown.
    pub artist: String,
    /// Album, empty if unknown.
    pub album: String,
    /// Cover art, if the file has some.
    pub art: Option<Art>,
    /// Length, if known.
    pub duration_us: Option<Timestamp>,
    /// The item has a picture (a video), not only sound.
    pub has_video: bool,
}

/// Transport state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayState {
    /// Nothing is loaded or playback was stopped.
    #[default]
    Stopped,
    /// Playing (including waiting for data).
    Playing,
    /// Paused.
    Paused,
}

/// Where playback is. The host extrapolates the position from `rate` between updates, so the player only sends a
/// new value when something changed (state, rate, a seek, the available commands).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Playback {
    /// Transport state.
    pub state: PlayState,
    /// Position when this was sent, microseconds.
    pub position_us: Timestamp,
    /// Playback speed (1.0 = normal).
    pub rate: f32,
    /// There is an item after this one.
    pub can_next: bool,
    /// There is an item before this one.
    pub can_prev: bool,
    /// Seeking works (the length is known).
    pub can_seek: bool,
}

/// A request from outside the app (media keys, a lock-screen widget, a desktop shell, `playerctl`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransportCommand {
    /// Start or resume.
    Play,
    /// Pause.
    Pause,
    /// Toggle play and pause.
    Toggle,
    /// Stop and go back to the start.
    Stop,
    /// Next item.
    Next,
    /// Previous item.
    Prev,
    /// Seek to this position, microseconds.
    SeekTo(Timestamp),
    /// Seek by this much (negative = back), microseconds.
    SeekBy(Timestamp),
    /// Playback speed.
    SetRate(f32),
    /// Volume, 0.0..=1.0.
    SetVolume(f32),
}

/// The now-playing model: the player reports what is playing and where, the host mirrors it to its OS or shell and
/// passes back transport commands. Optional: hosts without a media shell do not implement it.
pub trait NowPlaying {
    /// A different item is playing (or its tags became known).
    fn set_metadata(&mut self, meta: &NowPlayingMeta);
    /// The transport state or position changed.
    fn set_playback(&mut self, playback: &Playback);
    /// The player's volume (0.0..=1.0, 0 while muted) changed, whoever changed it: sent once at the start and then on every change, so a
    /// client that reads it back (MPRIS `Volume`) sees what the player does. Hosts without a volume property ignore it.
    fn set_volume(&mut self, _volume: f32) {}
    /// The next pending command from outside, if any.
    fn poll_command(&mut self) -> Option<TransportCommand>;
}

/// Number of log-spaced bands in a [`VizSummary`].
pub const VIZ_BANDS: usize = 32;

/// A stretch of the audio being heard, as the output device received it: interleaved `f32` before volume.
#[derive(Debug, Clone, Copy)]
pub struct VizBlock<'a> {
    /// Stream time of the first frame, microseconds.
    pub pts_us: Timestamp,
    /// Sample rate of `samples`, Hz.
    pub sample_rate: u32,
    /// Interleaved channels.
    pub channels: u16,
    /// The samples.
    pub samples: &'a [f32],
}

/// What a visualizer needs, computed by the core from the audio being heard (about 94 per second).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VizSummary {
    /// Stream time of the start of the analysed block, microseconds.
    pub pts_us: Timestamp,
    /// RMS level of the block, 0.0..=1.0.
    pub level: f32,
    /// Peak of the block, 0.0..=1.0.
    pub peak: f32,
    /// Magnitude per log-spaced band from about 30 Hz up, 0.0..=1.0 on a -70..0 dB scale.
    pub bands: [f32; VIZ_BANDS],
    /// Mean of the lowest quarter of the bands.
    pub bass: f32,
    /// Mean of the middle half of the bands.
    pub mid: f32,
    /// Mean of the highest quarter of the bands.
    pub treble: f32,
    /// A note, drum hit or other onset begins in this block.
    pub onset: bool,
    /// How strong the onset is relative to the recent average (0 when `onset` is false).
    pub onset_strength: f32,
    /// Estimated tempo in beats per minute, 0.0 while unknown.
    pub tempo_bpm: f32,
}

/// The visualizer tap: per-block PCM and an FFT/beat summary of what is being heard. Optional: the core only does
/// the analysis when the host provides one.
pub trait VisualizerTap {
    /// A block of audio as heard (pre-volume, so a quiet setting does not blank the picture).
    fn push_block(&mut self, block: &VizBlock<'_>);
    /// The analysis of the audio just passed to [`VisualizerTap::push_block`] (zero or more per block).
    fn push_summary(&mut self, summary: &VizSummary);
}

/// A tap that stores what it receives, for tests.
#[derive(Debug, Default)]
pub struct RecordingTap {
    /// Total frames received.
    pub frames: u64,
    /// Every summary received.
    pub summaries: Vec<VizSummary>,
}

impl VisualizerTap for RecordingTap {
    fn push_block(&mut self, block: &VizBlock<'_>) {
        self.frames += (block.samples.len() / block.channels.max(1) as usize) as u64;
    }

    fn push_summary(&mut self, summary: &VizSummary) {
        self.summaries.push(*summary);
    }
}

/// A now-playing sink that stores what it receives and replays queued commands, for tests.
#[derive(Debug, Default)]
pub struct RecordingNowPlaying {
    /// Every metadata update.
    pub metadata: Vec<NowPlayingMeta>,
    /// Every playback update.
    pub playback: Vec<Playback>,
    /// Every volume update.
    pub volumes: Vec<f32>,
    /// Commands waiting to be polled.
    pub commands: alloc::collections::VecDeque<TransportCommand>,
}

impl NowPlaying for RecordingNowPlaying {
    fn set_metadata(&mut self, meta: &NowPlayingMeta) {
        self.metadata.push(meta.clone());
    }

    fn set_playback(&mut self, playback: &Playback) {
        self.playback.push(*playback);
    }

    fn set_volume(&mut self, volume: f32) {
        self.volumes.push(volume);
    }

    fn poll_command(&mut self) -> Option<TransportCommand> {
        self.commands.pop_front()
    }
}
