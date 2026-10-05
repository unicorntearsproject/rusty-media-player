//! The library side of the application: loading and saving the index, running scans, the queue operations behind "play album",
//! "play next" and "add to queue", playlists (including importing and exporting files), what is playing (tags and cover art), the
//! visualizer's feed, and the switch between the Library and the Player face.
use super::{App, Effect};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;
use rvp_core::{Error, Timestamp};
use rvp_host::{FrameSink, Host, OpenRequest, Storage};
use rvp_library::{
    INDEX_KEY, Image, Library, ListFormat, PLAYLISTS_KEY, ScanEvent, ScanReport, ScanStatus, Scanner,
    art_key, encode_thumb,
};
use rvp_player::exec::Executor;
use rvp_ui::{Enqueue, LibAction, LibCtx, Mode, PlaylistEntry, Scope, UiCommand, View};
use rvp_viz::{FrameInput, Viz};

/// Longest side of the cover shown on the now-playing screen, pixels.
const NOW_ART_SIDE: u32 = 640;
/// Playlist files bigger than this are not read.
const MAX_PLAYLIST_FILE: usize = 8 << 20;
/// Pictures loaded from storage per tick.
const ART_PER_TICK: usize = 12;
/// Playback shorter than this and without a picture does not resume from where it stopped (a song, not an audiobook).
pub(crate) const RESUME_MIN_AUDIO_US: Timestamp = 20 * 60 * 1_000_000;
/// The visualizer picture is made at most this often, microseconds (about 30 a second).
const VIZ_EVERY_US: Timestamp = 30_000;

/// What is playing, as its tags say.
#[derive(Default)]
pub(crate) struct NowMeta {
    /// `(session tag, opened)` the data was read for.
    pub key: Option<(u32, bool)>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art: Option<Image>,
}

/// What the cached queue entries were built for: playlist revision, current item, library revision.
type QueueKey = (u32, Option<u32>, u64);

type Imports = Rc<RefCell<Vec<(String, Result<Vec<u8>, Error>)>>>;

/// Everything the library face adds to the application.
pub(crate) struct LibState {
    pub lib: Library,
    pub scanner: Scanner,
    pub loaded: bool,
    pub viz: Viz,
    pub viz_scope: Vec<f32>,
    pub viz_last_us: Timestamp,
    pub viz_frame: u64,
    pub now: NowMeta,
    pub imports: Imports,
    pub tasks: Executor,
    /// A file was opened from outside the library: when it is open, switch to the face that suits it.
    pub auto_mode: bool,
    pub scan_status: Option<ScanStatus>,
    pub last_scan_progress: usize,
    /// Pictures that storage did not have (not asked for again).
    pub missing_art: alloc::collections::BTreeSet<u64>,
    pub queue_cache: Option<(QueueKey, Rc<Vec<PlaylistEntry>>)>,
}

impl LibState {
    pub(crate) fn new() -> Self {
        Self {
            lib: Library::new(),
            scanner: Scanner::new(),
            loaded: false,
            viz: Viz::new(),
            viz_scope: Vec::new(),
            viz_last_us: 0,
            viz_frame: 0,
            now: NowMeta::default(),
            imports: Rc::default(),
            tasks: Executor::new(),
            auto_mode: false,
            scan_status: None,
            last_scan_progress: usize::MAX,
            missing_art: Default::default(),
            queue_cache: None,
        }
    }
}

/// A file name without its extension.
fn stem(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && e.len() <= 5 => s,
        _ => name,
    }
}

/// True for the names of playlist files.
pub(crate) fn is_playlist_name(name: &str) -> bool {
    ListFormat::from_name(name).is_some()
}

impl App {
    /// The library.
    pub fn library(&self) -> &Library {
        &self.lib.lib
    }

    /// How much memory the cover thumbnails may use (0: the default, 32 MiB); see [`Library::set_thumb_budget`].
    pub fn set_thumb_budget(&mut self, bytes: usize) {
        self.lib.lib.set_thumb_budget(bytes);
    }

    /// The visualizer (its picture and state).
    pub fn viz(&self) -> &rvp_viz::Viz {
        &self.lib.viz
    }

    /// Scan progress, if a scan is running.
    pub fn scan_status(&self) -> Option<&ScanStatus> {
        self.lib.scan_status.as_ref()
    }

    /// What the library face draws from.
    pub(crate) fn lib_ctx<'a>(lib: &'a LibState, video: Option<(&'a [u8], u32, u32)>) -> LibCtx<'a> {
        LibCtx {
            lib: &lib.lib,
            now_art: lib.now.art.as_ref(),
            scan: lib.scan_status.as_ref(),
            viz: Some(&lib.viz),
            video,
        }
    }

    // ---- loading and saving ---------------------------------------------------------------------------------------------

    fn lib_load<H: Host<Video = FrameSink>>(&mut self, host: &mut H) {
        self.lib.loaded = true;
        if let Some(bytes) = rvp_core::task::block_on(host.storage().load(INDEX_KEY)) {
            if let Ok(l) = Library::load_index(&bytes) {
                self.lib.lib = l;
            }
        }
        if let Some(bytes) = rvp_core::task::block_on(host.storage().load(PLAYLISTS_KEY)) {
            let _ = self.lib.lib.load_playlists(&bytes);
        }
    }

    /// Thumbnails the tracks use that are still on disk, a few per tick.
    fn lib_load_art<H: Host<Video = FrameSink>>(&mut self, host: &mut H) {
        let wanted: Vec<u64> = self
            .lib
            .lib
            .wanted_art()
            .into_iter()
            .filter(|a| !self.lib.missing_art.contains(a))
            .take(ART_PER_TICK)
            .collect();
        for id in wanted {
            match rvp_core::task::block_on(host.storage().load(&art_key(id))) {
                Some(b) if !b.is_empty() && self.lib.lib.load_thumb(id, &b) => {}
                _ => {
                    self.lib.missing_art.insert(id);
                }
            }
        }
    }

    /// Save what changed: the index and its new thumbnails, the playlists.
    pub(crate) fn lib_save<H: Host<Video = FrameSink>>(&mut self, host: &mut H) {
        if self.lib.lib.index_dirty() {
            let bytes = self.lib.lib.save_index();
            rvp_core::task::block_on(host.storage().store(INDEX_KEY, &bytes));
            self.lib_save_art(host);
        }
        self.lib_save_playlists(host);
    }

    /// Save the thumbnails that are new, and delete the ones nothing uses any more.
    fn lib_save_art<H: Host<Video = FrameSink>>(&mut self, host: &mut H) {
        for (id, th) in self.lib.lib.take_unsaved_art() {
            rvp_core::task::block_on(host.storage().store(&art_key(id), &encode_thumb(&th)));
        }
        for id in self.lib.lib.take_dropped_art() {
            rvp_core::task::block_on(host.storage().store(&art_key(id), &[]));
        }
    }

    fn lib_save_playlists<H: Host<Video = FrameSink>>(&mut self, host: &mut H) {
        if self.lib.lib.playlists_dirty() {
            let bytes = self.lib.lib.save_playlists();
            rvp_core::task::block_on(host.storage().store(PLAYLISTS_KEY, &bytes));
        }
    }

    // ---- the tick ---------------------------------------------------------------------------------------------------------

    /// Everything the library does between two frames.
    pub(crate) fn lib_tick<H>(&mut self, host: &mut H, now: Timestamp)
    where
        H: Host<Video = FrameSink>,
        H::Source: 'static,
    {
        if !self.lib.loaded {
            self.lib_load(host);
        }
        // Folders the host has finished listing.
        let mut listings = Vec::new();
        if let Some(l) = host.library() {
            while let Some(x) = l.take_listing() {
                listings.push(x);
            }
        }
        for x in listings {
            self.lib.scanner.push(x);
        }
        if self.lib.scanner.busy() {
            if let ScanEvent::Finished(rep) = self.lib.scanner.tick(&mut self.lib.lib, host) {
                self.lib_scan_done(host, rep, now);
            }
        }
        self.lib.scan_status = self.lib.scanner.status();
        if self.lib.lib.thumbs_need_save() {
            // A big scan: save the new covers now so they can leave memory.
            self.lib_save_art(host);
        }
        self.lib_load_art(host);
        self.lib_imports(host, now);
        self.refresh_now_meta(now);
        self.auto_switch_mode();
        self.lib_viz_tick(now);
    }

    fn lib_scan_done<H: Host<Video = FrameSink>>(&mut self, host: &mut H, rep: ScanReport, now: Timestamp) {
        // A listing gives the files new ids: queue items that were not opened yet must follow.
        let fresh: Vec<(u32, String)> = self
            .playlist
            .items()
            .iter()
            .filter_map(|i| {
                let t = self.lib.lib.track(i.track?)?;
                (!t.src.is_empty() && t.src != i.source).then(|| (i.id, t.src.clone()))
            })
            .collect();
        for (id, src) in fresh {
            self.playlist.set_source(id, &src);
        }
        self.lib_save(host);
        let changed = rep.added + rep.changed + rep.removed;
        let msg = if changed == 0 {
            format!("Library is up to date: {} tracks", self.lib.lib.track_count())
        } else {
            format!(
                "Library updated: {} tracks ({} new, {} changed, {} gone)",
                self.lib.lib.track_count(),
                rep.added,
                rep.changed,
                rep.removed
            )
        };
        self.ui.show_toast(&msg, now);
        if rep.failed > 0 {
            self.ui.show_toast(
                &format!("{} files could not be read and were skipped.", rep.failed),
                now + 1_500_000,
            );
        }
    }

    // ---- what is playing ----------------------------------------------------------------------------------------------------

    /// Read the tags and cover of the item playing, once per item (and once it is open).
    fn refresh_now_meta(&mut self, now: Timestamp) {
        let _ = now;
        let Some(s) = &self.session else {
            if self.lib.now.key.is_some() {
                self.lib.now = NowMeta::default();
            }
            return;
        };
        let opened = s.duration_us().is_some() || s.state() != rvp_player::SessionState::Opening;
        let key = (s.tag(), opened);
        if self.lib.now.key == Some(key) {
            return;
        }
        if !opened {
            self.lib.now = NowMeta { key: Some(key), ..NowMeta::default() };
            return;
        }
        let m = s.metadata();
        let art = m.art.as_ref().and_then(|a| rvp_library::art::decode(&a.data, NOW_ART_SIDE));
        self.lib.now = NowMeta {
            key: Some(key),
            title: m.title.filter(|t| !t.is_empty()).unwrap_or_default(),
            artist: m.artist.unwrap_or_default(),
            album: m.album.unwrap_or_default(),
            art,
        };
    }

    /// A file opened from outside the library: audio goes to the Library face, video to the Player.
    fn auto_switch_mode(&mut self) {
        if !self.lib.auto_mode {
            return;
        }
        let Some(s) = &self.session else { return };
        if s.state() == rvp_player::SessionState::Opening && s.duration_us().is_none() {
            return;
        }
        self.lib.auto_mode = false;
        if s.state() == rvp_player::SessionState::Failed {
            return;
        }
        if s.container_has_video() {
            self.ui.set_mode(Mode::Player);
        } else {
            self.ui.show_view(View::NowPlaying);
        }
    }

    // ---- the visualizer ---------------------------------------------------------------------------------------------------------

    fn viz_wanted(&self) -> bool {
        self.ui.mode() == Mode::Library
            && self.ui.lib_state().viz_on()
            && matches!(self.ui.lib_state().view(), View::NowPlaying | View::Visualizer)
            && self.ui.lib_state().detail().is_none()
    }

    fn lib_viz_tick(&mut self, now: Timestamp) {
        let wanted = self.viz_wanted();
        let reduce = self.ui.config.reduce_motion;
        let playing = self.model.state.is_active();
        if let Some(s) = &mut self.session {
            s.set_viz_capture(wanted);
            if wanted {
                let (sums, scope) = s.take_viz();
                for sm in &sums {
                    self.lib.viz.feed(sm, reduce);
                }
                self.lib.viz_scope.clear();
                self.lib.viz_scope.extend_from_slice(scope);
            }
        }
        if !wanted {
            return;
        }
        // Big screens (a 2560x1440 canvas) take fewer pictures a second to keep the tick short.
        let (sw, sh) = self.ui.size();
        let every = if sw as u64 * sh as u64 > 2_200_000 { VIZ_EVERY_US * 3 / 2 } else { VIZ_EVERY_US };
        if now - self.lib.viz_last_us < every && self.lib.viz.frames() > 0 {
            return;
        }
        self.lib.viz_last_us = now;
        let (w, h) = self.ui.size();
        let scope = core::mem::take(&mut self.lib.viz_scope);
        let before = self.lib.viz.frames();
        self.lib.viz.render(
            w as usize,
            h as usize,
            &FrameInput { now_us: now, playing, reduce_motion: reduce, scope: &scope },
        );
        self.lib.viz_scope = scope;
        if self.lib.viz.frames() != before {
            self.lib.viz_frame += 1;
            self.base_dirty = true;
            self.force_draw = true;
        }
    }

    // ---- the queue ---------------------------------------------------------------------------------------------------------------

    /// The queue as the UI shows it, rebuilt only when the list, the current item or the library changed.
    pub(crate) fn queue_entries(&mut self) -> Rc<Vec<PlaylistEntry>> {
        let key = (self.playlist.revision(), self.playlist.current_id(), self.lib.lib.revision());
        if let Some((k, v)) = &self.lib.queue_cache {
            if *k == key {
                return v.clone();
            }
        }
        let cur = self.playlist.current_id();
        let v: Vec<PlaylistEntry> = self
            .playlist
            .items()
            .iter()
            .map(|i| {
                let t = i.track.and_then(|id| self.lib.lib.track(id));
                match t {
                    Some(t) => {
                        let mut sub = t.display_artist().to_string();
                        if !t.album.is_empty() {
                            sub = format!("{sub} \u{b7} {}", t.album);
                        }
                        PlaylistEntry {
                            id: i.id,
                            label: t.display_title().to_string(),
                            current: Some(i.id) == cur,
                            subtitle: sub,
                            duration_us: t.duration_us,
                            track: Some(t.id),
                            art: t.art,
                        }
                    }
                    None => PlaylistEntry {
                        id: i.id,
                        label: i.name.clone(),
                        current: Some(i.id) == cur,
                        subtitle: String::new(),
                        duration_us: 0,
                        track: i.track,
                        art: 0,
                    },
                }
            })
            .collect();
        let rc = Rc::new(v);
        self.lib.queue_cache = Some((key, rc.clone()));
        rc
    }

    /// Replace, extend or insert into the queue with library tracks.
    pub(crate) fn queue_tracks<H>(
        &mut self,
        host: &mut H,
        ids: &[u32],
        how: Enqueue,
        start: usize,
        now: Timestamp,
    ) where
        H: Host<Video = FrameSink>,
        H::Source: 'static,
    {
        let items: Vec<(u32, String, String)> = ids
            .iter()
            .filter_map(|&id| self.lib.lib.track(id))
            .filter(|t| !t.src.is_empty())
            .map(|t| (t.id, t.display_title().to_string(), t.src.clone()))
            .collect();
        if items.is_empty() {
            self.ui.show_toast(
                if ids.is_empty() {
                    "Nothing to play there."
                } else {
                    "Those files aren't reachable right now. Scan their folder again."
                },
                now,
            );
            return;
        }
        // Start from the clicked row, counted among the tracks that can play.
        let start = ids
            .iter()
            .take(start)
            .filter(|id| items.iter().any(|(i, ..)| i == *id))
            .count()
            .min(items.len() - 1);
        let idle = !self.model.has_media()
            || self.model.state == rvp_ui::MediaState::Failed
            || self.session.is_none();
        match how {
            Enqueue::Now | Enqueue::ShuffleNow => {
                self.save_resume(host, true);
                let was_shuffled = self.playlist.shuffle();
                if was_shuffled {
                    self.playlist.set_shuffle(false, 0);
                }
                self.playlist.clear();
                let mut first = 0;
                for (i, (id, name, src)) in items.iter().enumerate() {
                    let item = self.playlist.add_track(name, src, Some(*id));
                    if i == start {
                        first = item;
                    }
                }
                let mut pick = first;
                let shuffle = how == Enqueue::ShuffleNow || was_shuffled;
                if how == Enqueue::ShuffleNow {
                    let k = (now.unsigned_abs() as usize) % items.len();
                    pick = self.playlist.items()[k].id;
                }
                self.playlist.set_current(pick);
                if shuffle {
                    self.playlist.set_shuffle(true, now as u64);
                }
                self.lib.auto_mode = false;
                self.requeue();
                self.play_item(host, pick);
                if how == Enqueue::ShuffleNow {
                    self.ui.show_toast("Shuffling", now);
                }
            }
            Enqueue::Next => {
                let mut after = self.playlist.current_id();
                let mut first = None;
                for (id, name, src) in &items {
                    let item = self.playlist.insert_after(after, name, src, Some(*id));
                    first.get_or_insert(item);
                    after = Some(item);
                }
                self.requeue();
                if idle {
                    if let Some(f) = first {
                        self.lib.auto_mode = false;
                        self.play_item(host, f);
                    }
                } else {
                    self.ui.show_toast(&format!("Playing {} next", items.len()), now);
                }
            }
            Enqueue::Append => {
                let mut first = None;
                for (id, name, src) in &items {
                    let item = self.playlist.add_track(name, src, Some(*id));
                    first.get_or_insert(item);
                }
                self.requeue();
                if idle {
                    if let Some(f) = first {
                        self.lib.auto_mode = false;
                        self.play_item(host, f);
                    }
                } else {
                    self.ui.show_toast(&format!("Added {} to the queue", items.len()), now);
                }
            }
        }
    }

    // ---- actions of the library face -------------------------------------------------------------------------------------------

    fn lib_scope(&self, scope: Scope) -> (Vec<u32>, usize) {
        let ctx = Self::lib_ctx(&self.lib, None);
        (self.ui.scope_tracks(scope, &ctx, &self.model), self.ui.scope_start(scope, &ctx))
    }

    pub(crate) fn apply_lib<H>(&mut self, host: &mut H, a: LibAction, now: Timestamp)
    where
        H: Host<Video = FrameSink>,
        H::Source: 'static,
    {
        match a {
            LibAction::Play(scope, how) => {
                let (ids, start) = self.lib_scope(scope);
                self.queue_tracks(host, &ids, how, start, now);
            }
            LibAction::AddToPlaylist(pl, scope) => {
                let (ids, _) = self.lib_scope(scope);
                self.lib.lib.playlist_add(pl, &ids);
                let name = self.lib.lib.playlist(pl).map(|p| p.name.clone()).unwrap_or_default();
                self.ui.show_toast(
                    &format!("Added {} to {name}", crate::plural(ids.len(), "track", "tracks")),
                    now,
                );
                self.lib_save_playlists(host);
            }
            LibAction::NewPlaylistFrom(scope) => self.ui.ask_playlist_name(Some(scope)),
            LibAction::NewPlaylist => self.ui.ask_playlist_name(None),
            LibAction::AddFolder => self.effects.push(Effect::AddFolder),
            LibAction::Rescan(i) => {
                let roots: Vec<String> = self.lib.lib.roots().iter().map(|r| r.id.clone()).collect();
                if i == u32::MAX {
                    for r in roots {
                        self.effects.push(Effect::Rescan(r));
                    }
                } else if let Some(r) = roots.get(i as usize) {
                    self.effects.push(Effect::Rescan(r.clone()));
                }
                self.ui.show_toast("Scanning\u{2026}", now);
            }
            LibAction::ForgetFolder(i) => {
                if let Some(r) = self.lib.lib.roots().get(i as usize).map(|r| (r.id.clone(), r.name.clone()))
                {
                    self.lib.lib.remove_root(&r.0);
                    self.lib_save(host);
                    self.ui.show_toast(&format!("Removed {} from the library", r.1), now);
                    self.effects.push(Effect::Forget(r.0));
                }
            }
            LibAction::ImportPlaylist => self.effects.push(Effect::ImportPlaylist),
            LibAction::ExportPlaylist(id, pls) => {
                let fmt = if pls { ListFormat::Pls } else { ListFormat::M3u8 };
                if let (Some(text), Some(p)) =
                    (self.lib.lib.export_playlist(id, fmt), self.lib.lib.playlist(id))
                {
                    let name: String = p
                        .name
                        .chars()
                        .map(
                            |c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' },
                        )
                        .collect();
                    let file = format!("{}.{}", name.trim(), fmt.extension());
                    let mime = if pls { "audio/x-scpls" } else { "audio/x-mpegurl" };
                    self.effects.push(Effect::Download {
                        name: file.clone(),
                        mime: mime.into(),
                        data: text.into_bytes(),
                    });
                    self.ui.show_toast(&format!("Saved {file}"), now);
                }
            }
            LibAction::DeletePlaylist(id) => {
                let name = self.lib.lib.playlist(id).map(|p| p.name.clone()).unwrap_or_default();
                self.lib.lib.delete_playlist(id);
                self.lib_save_playlists(host);
                self.ui.show_toast(&format!("Deleted {name}"), now);
                if self.ui.lib_state().detail() == Some(rvp_ui::Detail::Playlist(id)) {
                    self.ui.go_back();
                }
            }
            LibAction::RenamePlaylist(id) => {
                let cur = self.lib.lib.playlist(id).map(|p| p.name.clone()).unwrap_or_default();
                self.ui.ask_playlist_rename(id, &cur);
            }
            LibAction::RemoveFromPlaylist(pl, idx) => {
                self.lib.lib.playlist_remove(pl, idx as usize);
                self.lib_save_playlists(host);
            }
            LibAction::MovePlaylistEntry(pl, idx, d) => {
                self.lib.lib.playlist_move(pl, idx as usize, d as i32);
                self.lib_save_playlists(host);
            }
            LibAction::QueueToNext(id) => {
                self.playlist.move_after(id, None);
                self.requeue();
                self.ui.show_toast("Playing next", now);
            }
            LibAction::SortTracks(by, asc) => self.ui.set_track_sort(by, asc),
            LibAction::VizStep(d) => {
                self.lib.viz.effect = self.lib.viz.effect.step(d as i32);
                self.base_dirty = true;
                self.ui.show_toast(self.lib.viz.effect.name(), now);
                self.lib.viz_last_us = 0;
            }
            LibAction::VizPalette => {
                self.lib.viz.palette = self.lib.viz.palette.next();
                self.ui.show_toast(self.lib.viz.palette.name(), now);
                self.lib.viz_last_us = 0;
            }
            LibAction::VizInfo => {
                let on = !self.ui.lib_state().viz_info();
                self.ui.set_viz_info(on);
            }
            LibAction::VizToggle => {
                let on = !self.ui.lib_state().viz_on();
                self.ui.set_viz_on(on);
                self.base_dirty = true;
                self.lib.viz_last_us = 0;
            }
        }
    }

    /// Text the UI collected (names typed into prompts).
    pub(crate) fn drain_ui_commands<H>(&mut self, host: &mut H, now: Timestamp)
    where
        H: Host<Video = FrameSink>,
    {
        for c in self.ui.take_commands() {
            match c {
                UiCommand::CreatePlaylist { name, from } => {
                    let tracks = from.map(|s| self.lib_scope(s).0).unwrap_or_default();
                    let id = self.lib.lib.create_playlist(&name);
                    if !tracks.is_empty() {
                        self.lib.lib.playlist_add(id, &tracks);
                    }
                    let shown = self.lib.lib.playlist(id).map(|p| p.name.clone()).unwrap_or(name);
                    self.ui.show_toast(&format!("Made playlist {shown}"), now);
                    self.lib_save_playlists(host);
                }
                UiCommand::RenamePlaylist { id, name } => {
                    self.lib.lib.rename_playlist(id, &name);
                    self.lib_save_playlists(host);
                }
            }
        }
    }

    // ---- playlist files -----------------------------------------------------------------------------------------------------------

    /// Start reading a playlist file (the browser reads asynchronously, so this is a task); the result is filed by `lib_imports`.
    pub(crate) fn import_playlist_file<H>(&mut self, host: &mut H, id: &str, name: &str, now: Timestamp)
    where
        H: Host<Video = FrameSink>,
        H::Source: 'static,
    {
        match rvp_core::task::block_on(host.open(OpenRequest::Id(id.to_string()))) {
            Ok(src) => {
                let out = self.lib.imports.clone();
                let name = name.to_string();
                self.lib.tasks.spawn(async move {
                    let r = rvp_library::read_all(src, MAX_PLAYLIST_FILE).await;
                    out.borrow_mut().push((name, r));
                });
            }
            Err(e) => self.ui.show_toast(&format!("Couldn't open that: {e}"), now),
        }
    }

    /// Read a playlist file that came as an open source.
    pub(crate) fn import_playlist_source<S: rvp_host::Source + 'static>(&mut self, src: S, name: &str) {
        let out = self.lib.imports.clone();
        let name = name.to_string();
        self.lib.tasks.spawn(async move {
            let r = rvp_library::read_all(src, MAX_PLAYLIST_FILE).await;
            out.borrow_mut().push((name, r));
        });
    }

    fn lib_imports<H: Host<Video = FrameSink>>(&mut self, host: &mut H, now: Timestamp) {
        if !self.lib.tasks.is_idle() {
            self.lib.tasks.poll_all();
        }
        let done = core::mem::take(&mut *self.lib.imports.borrow_mut());
        for (name, r) in done {
            match r {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    let id = self.lib.lib.import_playlist(stem(&name), &text, "");
                    let (n, missing) =
                        self.lib.lib.playlist(id).map_or((0, 0), |p| (p.entries.len(), p.missing()));
                    let msg = if missing == 0 {
                        format!("Imported {} ({})", stem(&name), crate::plural(n, "track", "tracks"))
                    } else {
                        format!("Imported {}: {} missing of {}", stem(&name), missing, n)
                    };
                    self.ui.show_toast(&msg, now);
                    self.lib_save_playlists(host);
                    self.ui.show_view(View::Playlists);
                    self.ui.open_detail(rvp_ui::Detail::Playlist(id));
                }
                Err(e) => {
                    self.ui.show_toast(&format!("Couldn't read {name}: {}", crate::friendly_error(&e)), now)
                }
            }
        }
    }
}
