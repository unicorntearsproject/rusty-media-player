//! The host trait set: everything the player needs from its environment (browser page, Rusty Bucket
//! app runtime, headless test harness). See `docs/PLAN.md` section 4.
//!
//! All I/O is `async`, polled by the player's own cooperative executor, so a browser `Promise` and a
//! blocking native read look the same. The core never blocks, spawns, or reads a global clock.
#![no_std]
#![allow(async_fn_in_trait)]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod frame;
pub mod input;
pub mod library;
pub mod media;
pub mod mock;
pub mod net;
pub mod services;
pub mod types;
pub mod writer;

pub use frame::FrameSink;

pub use input::{InputEvent, Key, Modifiers, PointerButton, Rect};
pub use library::{FileEntry, Library, Listing, ScriptedLibrary, StandardFolder, StandardKind};
pub use media::{
    Art, NowPlaying, NowPlayingMeta, PlayState, Playback, RecordingNowPlaying, RecordingTap,
    TransportCommand, VIZ_BANDS, VisualizerTap, VizBlock, VizSummary,
};
pub use net::{MAX_FETCH_BYTES, Net, ScriptedNet};
pub use services::{
    AppServices, DefaultOutcome, DefaultPlayer, Integration, ScriptedServices, UpdateHow, UpdateState,
};
pub use types::{MEDIA_TYPES, MediaType, media_types_by_id, mimes_of};
pub use writer::{FileWriter, ScriptedWriter};

use alloc::{string::String, vec::Vec};
use rvp_core::{AudioParams, Timestamp, VideoFrame};

/// Host failure (I/O, device, permission).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostError(pub String);

impl core::fmt::Display for HostError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

impl core::error::Error for HostError {}

impl From<HostError> for rvp_core::Error {
    fn from(e: HostError) -> Self {
        rvp_core::Error::Host(e.0)
    }
}

/// Monotonic host time. Never goes backwards.
pub trait HostClock {
    /// Microseconds since an arbitrary epoch.
    fn now_us(&self) -> Timestamp;
    /// Ask the host to call `Player::tick` no later than `at_us`. The host may tick earlier.
    fn request_wake(&self, at_us: Timestamp);
    /// Seconds since 1970-01-01 UTC by the host's calendar clock, or 0 when the host has none (the play history then has no dates).
    fn unix_time(&self) -> i64 {
        0
    }
    /// Seconds the local time is ahead of UTC right now (negative to the west), 0 when unknown.
    fn utc_offset_secs(&self) -> i32 {
        0
    }
}

/// Random-access byte source (a file, a dropped `Blob`, a ramdisk entry).
pub trait Source {
    /// Total length in bytes, if known.
    async fn size(&self) -> Option<u64>;
    /// Read up to `buf.len()` bytes at `offset`; `Ok(0)` means end of data.
    async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, HostError>;
    /// Display name (file name).
    fn name(&self) -> &str;
}

/// Audio output. Samples are interleaved `f32`.
pub trait AudioSink {
    /// Open the device (again, for the next item after one without sound): this starts a new stream and nothing from an earlier
    /// one is left queued. The host may return different parameters than requested; the player adapts.
    fn open(&mut self, want: AudioParams) -> Result<AudioParams, HostError>;
    /// Frames written but not yet played.
    fn queued_frames(&self) -> usize;
    /// Device plus host buffering delay in microseconds.
    fn output_latency_us(&self) -> Timestamp;
    /// Queue interleaved samples; returns the number of whole frames accepted.
    fn write(&mut self, interleaved: &[f32]) -> usize;
    /// Drop queued audio (seek, stop).
    fn flush(&mut self);
    /// Pause or resume the device.
    fn set_paused(&mut self, paused: bool);
    /// Output volume, 0.0..=1.0.
    fn set_volume(&mut self, volume: f32);
}

/// Where decoded video goes.
pub trait VideoSink {
    /// Show `frame` in the video area of the window.
    fn present(&mut self, frame: &VideoFrame);
}

/// The UI canvas.
pub trait Surface {
    /// Width, height in physical pixels and the device pixel ratio.
    fn size(&self) -> (u32, u32, f32);
    /// Upload an RGBA8 framebuffer (`size().0 * size().1 * 4` bytes), only `dirty` needs to change.
    fn present_rgba(&mut self, rgba: &[u8], dirty: Rect);
    /// Enter or leave fullscreen.
    fn set_fullscreen(&mut self, on: bool);
}

/// Keyboard and pointer input.
pub trait InputEvents {
    /// Next pending event, if any.
    fn poll(&mut self) -> Option<InputEvent>;
}

/// Small persistent key-value store (settings, recents, resume positions).
pub trait Storage {
    /// Load a value.
    async fn load(&mut self, key: &str) -> Option<Vec<u8>>;
    /// Store a value.
    async fn store(&mut self, key: &str, value: &[u8]);
}

/// What to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenRequest {
    /// Ask the user (file picker).
    Pick,
    /// A host-defined identifier (path, dropped-file handle, URL).
    Id(String),
}

/// The bundle of capabilities a player is run with.
pub trait Host {
    /// Byte source type.
    type Source: Source;
    /// Audio sink type.
    type Audio: AudioSink;
    /// Video sink type.
    type Video: VideoSink;
    /// Storage type.
    type Store: Storage;
    /// Monotonic clock.
    fn clock(&self) -> &dyn HostClock;
    /// Audio output.
    fn audio(&mut self) -> &mut Self::Audio;
    /// Video output.
    fn video(&mut self) -> &mut Self::Video;
    /// UI canvas.
    fn surface(&mut self) -> &mut dyn Surface;
    /// Input events.
    fn input(&mut self) -> &mut dyn InputEvents;
    /// Persistent storage.
    fn storage(&mut self) -> &mut Self::Store;
    /// Open a media source.
    async fn open(&mut self, req: OpenRequest) -> Result<Self::Source, HostError>;
    /// The host's now-playing integration (Media Session, MPRIS, a shell), if it has one.
    fn now_playing(&mut self) -> Option<&mut dyn NowPlaying> {
        None
    }
    /// The host's visualizer tap, if it wants the audio analysis.
    fn visualizer(&mut self) -> Option<&mut dyn VisualizerTap> {
        None
    }
    /// Directory access for the library view, if the host has any.
    fn library(&mut self) -> Option<&mut dyn Library> {
        None
    }
    /// Fetching a page of text the user pointed the app at (a link to a design system), if the host can.
    fn net(&mut self) -> Option<&mut dyn Net> {
        None
    }
    /// Replacing library files (editing tags), if the host can.
    fn file_writer(&mut self) -> Option<&mut dyn FileWriter> {
        None
    }
    /// Update checks and adding the app to the desktop's menus, if the host has them.
    fn app_services(&mut self) -> Option<&mut dyn AppServices> {
        None
    }
    /// True when the host can open a web page in the person's browser (an `https` link on the About page); where it cannot, the page
    /// shows the address as text.
    fn opens_links(&self) -> bool {
        false
    }
    /// True when the ids this host gives to [`OpenRequest::Id`] still open the same file after a restart (file paths).
    /// The player then keeps them in the saved queue; a host whose ids belong to one session (a browser's stashed
    /// `File` objects) keeps the default and only library tracks are restored, through the library's own listing.
    fn stable_ids(&self) -> bool {
        false
    }
}
