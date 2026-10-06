//! The Rusty Bucket [`Host`]: the clock, audio, canvas, input, storage, files, now-playing, visualizer and library of the App API
//! wired together. The driver ([`crate::RbPlayer`]) feeds it events and owns the app.
use crate::api;
use crate::audio::RbAudio;
use crate::clock::RbClock;
use crate::library::RbLibrary;
use crate::media::{RbNowPlaying, RbViz};
use crate::shared::Shared;
use crate::source::RbSource;
use crate::storage::RbStorage;
use crate::surface::RbSurface;
use bucket_v0_sys::{self as sys, caps, err};
use rvp_host::{
    FrameSink, Host, HostClock, HostError, InputEvent, InputEvents, Library, NowPlaying, OpenRequest,
    Surface, VisualizerTap,
};
use std::collections::VecDeque;
use std::rc::Rc;

/// How long `file_open_id` that is `-BUSY` is waited for, microseconds.
const OPEN_TIMEOUT_US: i64 = 5_000_000;

/// Input events the driver decoded, waiting for the app.
#[derive(Default)]
pub struct RbInput(pub VecDeque<InputEvent>);

impl InputEvents for RbInput {
    fn poll(&mut self) -> Option<InputEvent> {
        self.0.pop_front()
    }
}

/// Everything the app needs from Rusty Bucket.
pub struct RbHost {
    /// What the parts share.
    pub shared: Rc<Shared>,
    /// The clock.
    pub clock: RbClock,
    /// Audio output.
    pub audio: RbAudio,
    /// The latest picture, converted to RGBA for the app to compose (plan A).
    pub video: FrameSink,
    /// The canvas.
    pub surface: RbSurface,
    /// Pending input.
    pub input: RbInput,
    /// The key-value store.
    pub storage: RbStorage,
    /// Library roots and listings.
    pub library: RbLibrary,
    /// The shell's now-playing interface.
    pub now_playing: RbNowPlaying,
    /// The visualizer feed.
    pub viz: RbViz,
}

impl RbHost {
    /// Read the capabilities and limits and set everything up.
    pub fn new() -> Self {
        Self::with_shared(Shared::new())
    }

    /// With given shared state (tests).
    pub fn with_shared(shared: Rc<Shared>) -> Self {
        let limits = shared.limits;
        Self {
            clock: RbClock::new(),
            audio: RbAudio::new(),
            video: FrameSink::new(),
            surface: RbSurface::new(),
            input: RbInput::default(),
            storage: RbStorage::new(shared.clone()),
            library: RbLibrary::new(),
            now_playing: RbNowPlaying::new(limits),
            viz: RbViz::new(),
            shared,
        }
    }

    /// Open a file by the id the player has for it: a stashed handle (a file that has no stable id) or `file_open_id`.
    fn open_id(&self, id: &str) -> Result<RbSource, HostError> {
        let max_read = self.shared.limits.read;
        if let Some(h) = self.shared.stash.borrow_mut().remove(id) {
            return Ok(RbSource::from_handle(h, max_read));
        }
        let deadline = api::now_us() + OPEN_TIMEOUT_US;
        loop {
            // SAFETY: the id is the string's range.
            let h = unsafe { sys::file_open_id(id.as_ptr(), api::len32(id.len())) };
            match h {
                h if h > 0 => return Ok(RbSource::from_handle(h, max_read)),
                err::BUSY => {
                    let left = deadline - api::now_us();
                    if left <= 0 || !self.shared.wait_io(left) {
                        return Err(HostError("the file took too long to open".into()));
                    }
                }
                err::NOT_FOUND => return Err(HostError("that file is not there any more".into())),
                e => return Err(HostError(format!("cannot open the file ({})", api::code_name(e)))),
            }
        }
    }
}

impl Default for RbHost {
    fn default() -> Self {
        Self::new()
    }
}

impl Host for RbHost {
    type Source = RbSource;
    type Audio = RbAudio;
    type Video = FrameSink;
    type Store = RbStorage;

    fn clock(&self) -> &dyn HostClock {
        &self.clock
    }
    fn audio(&mut self) -> &mut RbAudio {
        &mut self.audio
    }
    fn video(&mut self) -> &mut FrameSink {
        &mut self.video
    }
    fn surface(&mut self) -> &mut dyn Surface {
        &mut self.surface
    }
    fn input(&mut self) -> &mut dyn InputEvents {
        &mut self.input
    }
    fn storage(&mut self) -> &mut RbStorage {
        &mut self.storage
    }
    async fn open(&mut self, req: OpenRequest) -> Result<RbSource, HostError> {
        match req {
            OpenRequest::Id(id) => self.open_id(&id),
            OpenRequest::Pick => Err(HostError("the shell shows its own file picker".into())),
        }
    }
    fn now_playing(&mut self) -> Option<&mut dyn NowPlaying> {
        self.shared.has(caps::NOW_PLAYING).then_some(&mut self.now_playing as &mut dyn NowPlaying)
    }
    fn visualizer(&mut self) -> Option<&mut dyn VisualizerTap> {
        self.shared.has(caps::VISUALIZER).then_some(&mut self.viz as &mut dyn VisualizerTap)
    }
    fn library(&mut self) -> Option<&mut dyn Library> {
        self.shared.has(caps::LIBRARY).then_some(&mut self.library as &mut dyn Library)
    }
    /// Every id `file_id` gives reopens the same file after a restart.
    fn stable_ids(&self) -> bool {
        true
    }
}
