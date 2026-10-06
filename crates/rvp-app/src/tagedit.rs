//! Editing tags: the form's content from the library, and the job that follows a Save. For each song the file is read, edited by
//! `rvp-tagwrite`, checked (the edited bytes must still read as the same audio) and only then handed to the host to replace the file
//! (safely, whole, or not at all). The library shows the change at once and the folder is read again to confirm it.
use super::{App, Effect};
use alloc::collections::{BTreeSet, VecDeque};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;
use rvp_core::Timestamp;
use rvp_host::{FrameSink, Host, OpenRequest, Source};
use rvp_library::{TagPatch, Track, TrackId, read_all, read_tags};
use rvp_tagwrite::{Change, Cover, Edit, edit_tags, format_of};
use rvp_ui::{CoverAction, Scope, TagField, TagFormSpec, TagTarget};

/// Files bigger than this are not edited (they are held in memory, the old and the new one at once: in a page, with its 4 GB address
/// space shared by everything else, far less than on a desktop).
#[cfg(target_arch = "wasm32")]
const MAX_EDIT_FILE: usize = 64 << 20;
/// Files bigger than this are not edited (they are held in memory, the old and the new one at once).
#[cfg(not(target_arch = "wasm32"))]
const MAX_EDIT_FILE: usize = 256 << 20;
/// Cover pictures bigger than this are not accepted.
const MAX_COVER: usize = 8 << 20;

type Prepared = (TrackId, Result<Vec<u8>, String>);

struct Item {
    id: TrackId,
    root: String,
    path: String,
    src: String,
    name: String,
    want_us: i64,
}

enum Stage {
    Idle,
    Preparing(Item),
    Writing(Item, u32),
}

struct Job {
    edit: Edit,
    patch: TagPatch,
    queue: VecDeque<Item>,
    total: usize,
    done: usize,
    failed: Vec<(String, String)>,
    stage: Stage,
    roots: BTreeSet<String>,
}

/// The tag editor's state in the application.
#[derive(Default)]
pub(crate) struct TagState {
    /// The picture the person chose for the cover: its type and bytes.
    cover: Option<(String, Vec<u8>)>,
    job: Option<Job>,
    prepared: Rc<RefCell<Vec<Prepared>>>,
}

/// A source over bytes that someone else keeps (the edited file is checked where it is, not in a copy).
struct SharedBytes(Rc<Vec<u8>>);

impl Source for SharedBytes {
    async fn size(&self) -> Option<u64> {
        Some(self.0.len() as u64)
    }

    async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, rvp_host::HostError> {
        let start = (offset as usize).min(self.0.len());
        let n = buf.len().min(self.0.len() - start);
        buf[..n].copy_from_slice(&self.0[start..start + n]);
        Ok(n)
    }

    fn name(&self) -> &str {
        "edited"
    }
}

/// Read, edit and check one file; the bytes to write, or why not. The old bytes are let go as soon as the new ones exist, and the check
/// reads the new ones in place, so at most two copies of the file are ever held (not three).
async fn prepare<S: Source>(src: S, name: &str, edit: &Edit, want_us: i64) -> Result<Vec<u8>, String> {
    let old = read_all(src, MAX_EDIT_FILE).await.map_err(|e| {
        format!(
            "it could not be read, or it is bigger than the {} MB that can be edited here ({e})",
            MAX_EDIT_FILE >> 20
        )
    })?;
    let fmt = format_of(name, &old[..old.len().min(16)])
        .ok_or_else(|| "tags in this kind of file cannot be written".to_string())?;
    let new = edit_tags(fmt, &old, edit).map_err(|e| e.to_string())?;
    drop(old);
    // The edited file has to read as the same audio, or nothing is written.
    let shared = Rc::new(new);
    let t = read_tags(SharedBytes(shared.clone()))
        .await
        .map_err(|_| "the edited file would not read back, so nothing was written".to_string());
    let new = Rc::try_unwrap(shared).unwrap_or_else(|rc| (*rc).clone());
    let t = t?;
    if want_us > 0 && t.duration_us > 0 && (t.duration_us - want_us).abs() > want_us / 50 + 100_000 {
        return Err("the edited file reads as a different length, so nothing was written".into());
    }
    Ok(new)
}

fn common(recs: &[&Track], f: impl Fn(&Track) -> String) -> (String, bool) {
    let first = f(recs[0]);
    if recs.iter().all(|t| f(t) == first) { (first, false) } else { (String::new(), true) }
}

fn num(n: u32) -> String {
    if n == 0 { String::new() } else { n.to_string() }
}

impl App {
    /// The root id (the host's) of the folder a track is in.
    fn track_root(&self, t: &Track) -> Option<String> {
        self.lib.lib.roots().get(t.root as usize).map(|r| r.id.clone())
    }

    fn scope_tracks(&self, scope: Scope) -> Vec<TrackId> {
        match scope {
            Scope::Track(id) => alloc::vec![id],
            Scope::Album(a) => self.lib.lib.album(a).map(|a| a.tracks.clone()).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Open the tag editor for a song or the shared tags of an album.
    pub(crate) fn open_tag_form<H>(&mut self, host: &mut H, scope: Scope, now: Timestamp)
    where
        H: Host<Video = FrameSink>,
    {
        let ids = self.scope_tracks(scope);
        let lib = &self.lib.lib;
        let recs: Vec<&Track> = ids.iter().filter_map(|&i| lib.track(i)).collect();
        if recs.is_empty() {
            return;
        }
        if self.lib.tags.job.is_some() {
            self.ui.show_toast("Still saving the last edit", now);
            return;
        }
        let root = self.track_root(recs[0]).unwrap_or_default();
        let read_only = match host.file_writer() {
            None => Some("Rusty Wave cannot change files here, so tags cannot be edited.".to_string()),
            Some(w) => w.can_write(&root).err(),
        };
        let (title, subtitle, target, fields) = match scope {
            Scope::Album(a) => {
                let name = lib.album(a).map(|a| a.title.clone()).unwrap_or_default();
                let f = |k: TagField, (v, mixed): (String, bool)| (k, v, mixed);
                (
                    "Edit album tags".to_string(),
                    format!("{} \u{b7} {name}", crate::plural(recs.len(), "song", "songs")),
                    TagTarget::Album(a),
                    alloc::vec![
                        f(TagField::Artist, common(&recs, |t| t.artist.clone())),
                        f(TagField::Album, common(&recs, |t| t.album.clone())),
                        f(TagField::AlbumArtist, common(&recs, |t| t.album_artist.clone())),
                        f(TagField::Genre, common(&recs, |t| t.genre.clone())),
                        f(
                            TagField::Year,
                            common(&recs, |t| if t.year > 0 { t.year.to_string() } else { String::new() })
                        ),
                        f(TagField::DiscTotal, common(&recs, |t| num(t.disc_total as u32))),
                    ],
                )
            }
            _ => {
                let t = recs[0];
                (
                    "Edit tags".to_string(),
                    t.file_name().to_string(),
                    TagTarget::Track(t.id),
                    alloc::vec![
                        (TagField::Title, t.title.clone(), false),
                        (TagField::Artist, t.artist.clone(), false),
                        (TagField::Album, t.album.clone(), false),
                        (TagField::AlbumArtist, t.album_artist.clone(), false),
                        (TagField::Genre, t.genre.clone(), false),
                        (TagField::TrackNo, num(t.track_no as u32), false),
                        (TagField::TrackTotal, num(t.track_total as u32), false),
                        (TagField::DiscNo, num(t.disc_no as u32), false),
                        (TagField::DiscTotal, num(t.disc_total as u32), false),
                        (TagField::Year, if t.year > 0 { t.year.to_string() } else { String::new() }, false),
                    ],
                )
            }
        };
        let art = common_art(&recs);
        self.lib.tags.cover = None;
        self.ui.open_tag_form(TagFormSpec { title, subtitle, target, fields, art, read_only });
    }

    /// The Replace button asked the host for a picture and this is what it gave.
    pub fn cover_picked(&mut self, name: &str, bytes: Vec<u8>, now: Timestamp) {
        if !self.ui.tag_form_open() {
            return;
        }
        let mime = if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            "image/jpeg"
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image/png"
        } else {
            self.ui.show_toast("That is not a JPEG or a PNG picture", now);
            return;
        };
        if bytes.len() > MAX_COVER {
            self.ui.show_toast("That picture is too big (8 MB at most)", now);
            return;
        }
        self.lib.tags.cover = Some((mime.to_string(), bytes));
        self.ui.set_tag_cover(Some(name.to_string()));
    }

    /// The form was saved: start writing the songs.
    pub(crate) fn start_tag_job(
        &mut self,
        target: TagTarget,
        changes: Vec<(TagField, Option<String>)>,
        cover: CoverAction,
        now: Timestamp,
    ) {
        if self.lib.tags.job.is_some() {
            self.ui.show_toast("Still saving the last edit", now);
            return;
        }
        let ids = self.scope_tracks(match target {
            TagTarget::Track(i) => Scope::Track(i),
            TagTarget::Album(a) => Scope::Album(a),
        });
        let mut edit = Edit::default();
        let mut patch = TagPatch::default();
        for (field, value) in changes {
            let text = |v: &Option<String>| match v {
                Some(s) => Change::Set(s.clone()),
                None => Change::Clear,
            };
            let number = |v: &Option<String>| match v.as_deref().and_then(|s| s.trim().parse::<u32>().ok()) {
                Some(n) if n > 0 => Change::Set(n),
                _ => Change::Clear,
            };
            let short =
                |v: &Option<String>| v.as_deref().and_then(|s| s.trim().parse::<u16>().ok()).unwrap_or(0);
            let s = value.clone().unwrap_or_default();
            match field {
                TagField::Title => {
                    edit.title = text(&value);
                    patch.title = Some(s);
                }
                TagField::Artist => {
                    edit.artist = text(&value);
                    patch.artist = Some(s);
                }
                TagField::Album => {
                    edit.album = text(&value);
                    patch.album = Some(s);
                }
                TagField::AlbumArtist => {
                    edit.album_artist = text(&value);
                    patch.album_artist = Some(s);
                }
                TagField::Genre => {
                    edit.genre = text(&value);
                    patch.genre = Some(s);
                }
                TagField::TrackNo => {
                    edit.track_no = number(&value);
                    patch.track_no = Some(short(&value));
                }
                TagField::TrackTotal => {
                    edit.track_total = number(&value);
                    patch.track_total = Some(short(&value));
                }
                TagField::DiscNo => {
                    edit.disc_no = number(&value);
                    patch.disc_no = Some(short(&value));
                }
                TagField::DiscTotal => {
                    edit.disc_total = number(&value);
                    patch.disc_total = Some(short(&value));
                }
                TagField::Year => {
                    edit.year = match value.as_deref().and_then(|s| s.trim().parse::<i32>().ok()) {
                        Some(y) if y > 0 => Change::Set(y),
                        _ => Change::Clear,
                    };
                    patch.year = Some(value.as_deref().and_then(|s| s.trim().parse().ok()).unwrap_or(0));
                }
            }
        }
        edit.cover = match (cover, self.lib.tags.cover.take()) {
            (CoverAction::Remove, _) => Change::Clear,
            (CoverAction::Replace, Some((mime, data))) => Change::Set(Cover { mime, data }),
            _ => Change::Keep,
        };
        if edit.is_empty() {
            return;
        }
        let mut queue = VecDeque::new();
        let mut roots = BTreeSet::new();
        let mut failed = Vec::new();
        for id in ids {
            let Some(t) = self.lib.lib.track(id) else { continue };
            let name = t.file_name().to_string();
            match self.track_root(t) {
                Some(root) if !t.src.is_empty() => {
                    roots.insert(root.clone());
                    queue.push_back(Item {
                        id,
                        root,
                        path: t.path.clone(),
                        src: t.src.clone(),
                        name,
                        want_us: t.duration_us,
                    });
                }
                _ => failed.push((name, "its folder is not connected right now".to_string())),
            }
        }
        let total = queue.len() + failed.len();
        self.lib.tags.job =
            Some(Job { edit, patch, queue, total, done: 0, failed, stage: Stage::Idle, roots });
        self.ui.show_toast("Saving tags\u{2026}", now);
    }

    /// Advance the job: a file at a time, each one read and checked before the host is asked to replace it.
    pub(crate) fn tag_tick<H>(&mut self, host: &mut H, now: Timestamp)
    where
        H: Host<Video = FrameSink>,
        H::Source: 'static,
    {
        let Some(mut job) = self.lib.tags.job.take() else { return };
        loop {
            match core::mem::replace(&mut job.stage, Stage::Idle) {
                Stage::Idle => {
                    let Some(item) = job.queue.pop_front() else { break };
                    match rvp_core::task::block_on(host.open(OpenRequest::Id(item.src.clone()))) {
                        Ok(src) => {
                            let out = self.lib.tags.prepared.clone();
                            let (name, edit, want, id) =
                                (item.name.clone(), job.edit.clone(), item.want_us, item.id);
                            self.lib.tasks.spawn(async move {
                                let r = prepare(src, &name, &edit, want).await;
                                out.borrow_mut().push((id, r));
                            });
                            job.stage = Stage::Preparing(item);
                        }
                        Err(_) => job.failed.push((item.name, "it could not be opened".into())),
                    }
                }
                Stage::Preparing(item) => {
                    let found = {
                        let mut p = self.lib.tags.prepared.borrow_mut();
                        p.iter().position(|(id, _)| *id == item.id).map(|i| p.remove(i).1)
                    };
                    match found {
                        None => {
                            job.stage = Stage::Preparing(item);
                            break;
                        }
                        Some(Err(why)) => job.failed.push((item.name, why)),
                        Some(Ok(bytes)) => match host.file_writer() {
                            Some(w) => {
                                let ticket = w.write(&item.root, &item.path, bytes);
                                job.stage = Stage::Writing(item, ticket);
                            }
                            None => job.failed.push((item.name, "this host cannot write files".into())),
                        },
                    }
                }
                Stage::Writing(item, ticket) => {
                    let answer = host.file_writer().and_then(|w| w.poll_write(ticket));
                    match answer {
                        None => {
                            job.stage = Stage::Writing(item, ticket);
                            break;
                        }
                        Some(Ok(())) => {
                            job.done += 1;
                            self.lib.lib.patch_tags(item.id, &job.patch);
                        }
                        Some(Err(why)) => job.failed.push((item.name, why)),
                    }
                }
            }
        }
        if matches!(job.stage, Stage::Idle) && job.queue.is_empty() {
            self.finish_tag_job(host, job, now);
        } else {
            self.lib.tags.job = Some(job);
        }
    }

    fn finish_tag_job<H>(&mut self, host: &mut H, job: Job, now: Timestamp)
    where
        H: Host<Video = FrameSink>,
    {
        self.lib_save(host);
        let _ = job.total;
        let msg = match (job.done, job.failed.first()) {
            (0, Some((name, why))) => format!("Could not save {name}: {why}"),
            (n, None) => format!("Saved the tags of {}", crate::plural(n, "song", "songs")),
            (n, Some((name, why))) => format!(
                "Saved {}; {} not saved ({name}: {why})",
                crate::plural(n, "song", "songs"),
                crate::plural(job.failed.len(), "song was", "songs were")
            ),
        };
        self.ui.show_toast(&msg, now);
        if job.done > 0 {
            // Read the folders again: what is on disk is the truth, and the library confirms what it showed.
            for root in job.roots {
                self.effects.push(Effect::Rescan(root));
            }
        }
    }
}

fn common_art(recs: &[&Track]) -> u64 {
    recs.iter().map(|t| t.art).find(|a| *a != 0).unwrap_or(0)
}
