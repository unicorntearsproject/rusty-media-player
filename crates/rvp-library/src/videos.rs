//! The video side of [`Library`]: the list, ordering, search, filing scan results, posters, and saving (`library/videos`).
use crate::art::Thumb;
use crate::fold::{fold, natural};
use crate::index::Library;
use crate::model::ArtId;
use crate::video::{Video, VideoId, VideoInfo, poster_id};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use rvp_core::Error;
use rvp_host::FileEntry;

/// Storage key of the videos.
pub const VIDEOS_KEY: &str = "library/videos";
const VIDEOS_MAGIC: &[u8; 4] = b"RVPV";
const VIDEOS_VERSION: u8 = 1;

/// How the video list is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoSort {
    /// By title.
    #[default]
    Title,
    /// By when the file was last changed (newest last when ascending).
    Added,
    /// By length.
    Length,
}

impl Library {
    /// Every remembered video file, including the ones that could not be read.
    pub fn all_videos(&self) -> &[Video] {
        &self.videos
    }

    /// Number of videos that are shown.
    pub fn video_count(&self) -> usize {
        self.videos.iter().filter(|v| !v.unreadable).count()
    }

    /// The video with `id`.
    pub fn video(&self, id: VideoId) -> Option<&Video> {
        self.videos.binary_search_by_key(&id, |v| v.id).ok().map(|i| &self.videos[i])
    }

    /// The shown videos in the given order.
    pub fn sorted_videos(&self, by: VideoSort, ascending: bool) -> Vec<VideoId> {
        let mut keyed: Vec<(String, VideoId)> = self
            .videos
            .iter()
            .filter(|v| !v.unreadable)
            .map(|v| {
                let key = match by {
                    VideoSort::Title => format!("{}\u{0}{:010}", natural(&fold(v.display_title())), v.id),
                    VideoSort::Added => format!("{:020}\u{0}{:010}", v.mtime_ms.max(0), v.id),
                    VideoSort::Length => format!("{:020}\u{0}{:010}", v.duration_us.max(0), v.id),
                };
                (key, v.id)
            })
            .collect();
        keyed.sort();
        if !ascending {
            keyed.reverse();
        }
        keyed.into_iter().map(|(_, id)| id).collect()
    }

    /// The shown videos whose title or folder contains every word (already folded) of a search.
    pub(crate) fn search_videos(&self, words: &[&str]) -> Vec<VideoId> {
        self.videos
            .iter()
            .filter(|v| !v.unreadable)
            .filter(|v| {
                let hay = fold(&format!("{} {}", v.display_title(), v.path));
                words.iter().all(|w| hay.contains(w))
            })
            .map(|v| v.id)
            .collect()
    }

    /// Total length of the shown videos.
    pub fn total_video_duration_us(&self) -> i64 {
        self.videos.iter().filter(|v| !v.unreadable).map(|v| v.duration_us).sum()
    }

    /// File the result of reading the headers of video `entry` (a file of root `root`).
    pub fn apply_video(&mut self, root: u16, entry: &FileEntry, result: Result<VideoInfo, Error>) {
        let existing = self.videos.iter().position(|v| v.root == root && v.path == entry.path);
        let id = match existing {
            Some(i) => self.videos[i].id,
            None => {
                let id = self.next_id.max(1);
                self.next_id = id + 1;
                id
            }
        };
        let mut v = Video {
            id,
            root,
            path: entry.path.clone(),
            size: entry.size,
            mtime_ms: entry.mtime_ms,
            title: String::new(),
            duration_us: 0,
            width: 0,
            height: 0,
            vcodec: String::new(),
            acodec: String::new(),
            poster: 0,
            poster_tried: false,
            unreadable: false,
            src: entry.id.clone(),
        };
        match result {
            Ok(i) => {
                v.title = i.title;
                v.duration_us = i.duration_us;
                v.width = i.width;
                v.height = i.height;
                v.vcodec = i.vcodec;
                v.acodec = i.acodec;
            }
            Err(_) => {
                v.unreadable = true;
                self.report.failed += 1;
            }
        }
        match existing {
            Some(i) => {
                // A changed file keeps no poster (the id follows the file's time and size, so the old one is simply unused).
                self.videos[i] = v;
                self.report.changed += 1;
            }
            None => {
                self.videos.push(v);
                self.videos.sort_by_key(|v| v.id);
                self.report.added += 1;
            }
        }
        self.videos_dirty = true;
    }

    /// Videos that have no poster yet and can be opened now: `(id, what the host opens it with)`.
    pub fn pending_posters(&self) -> Vec<(VideoId, String)> {
        self.videos
            .iter()
            .filter(|v| !v.unreadable && v.poster == 0 && !v.poster_tried && !v.src.is_empty())
            .map(|v| (v.id, v.src.clone()))
            .collect()
    }

    /// File the poster made for video `id` (`None`: no frame could be made, and it is not tried again until the file changes).
    pub fn set_poster(&mut self, id: VideoId, poster: Option<Thumb>) {
        let Ok(i) = self.videos.binary_search_by_key(&id, |v| v.id) else { return };
        match poster {
            Some(t) => {
                let v = &self.videos[i];
                let art = poster_id(v.root, &v.path, v.mtime_ms, v.size);
                self.insert_thumb(art, t);
                self.videos[i].poster = art;
            }
            None => self.videos[i].poster_tried = true,
        }
        self.videos_dirty = true;
        self.rev += 1;
    }

    /// Pictures the videos use (their posters).
    pub(crate) fn poster_art(&self) -> impl Iterator<Item = ArtId> + '_ {
        self.videos.iter().map(|v| v.poster).filter(|&a| a != 0)
    }

    /// True if the videos changed since [`Library::save_videos`] last ran.
    pub fn videos_dirty(&self) -> bool {
        self.videos_dirty
    }

    /// Drop the videos of root index `ri` and renumber the roots above it (a folder was forgotten).
    pub(crate) fn remove_root_videos(&mut self, ri: usize) {
        let before = self.videos.len();
        self.videos.retain(|v| v.root as usize != ri);
        for v in &mut self.videos {
            if v.root as usize > ri {
                v.root -= 1;
            }
        }
        if self.videos.len() != before {
            self.videos_dirty = true;
        }
    }

    /// The videos as bytes, and mark them saved.
    pub fn save_videos(&mut self) -> Vec<u8> {
        self.videos_dirty = false;
        let mut out = Vec::new();
        out.extend_from_slice(VIDEOS_MAGIC);
        out.push(VIDEOS_VERSION);
        // The roots they refer to, by id, so a changed folder list cannot point a video at the wrong folder.
        out.extend_from_slice(&(self.roots.len() as u16).to_le_bytes());
        for r in &self.roots {
            put_str(&mut out, &r.id);
        }
        out.extend_from_slice(&(self.videos.len() as u32).to_le_bytes());
        for v in &self.videos {
            out.extend_from_slice(&v.id.to_le_bytes());
            out.extend_from_slice(&v.root.to_le_bytes());
            put_str(&mut out, &v.path);
            out.extend_from_slice(&v.size.to_le_bytes());
            out.extend_from_slice(&v.mtime_ms.to_le_bytes());
            put_str(&mut out, &v.title);
            out.extend_from_slice(&v.duration_us.to_le_bytes());
            out.extend_from_slice(&v.width.to_le_bytes());
            out.extend_from_slice(&v.height.to_le_bytes());
            put_str(&mut out, &v.vcodec);
            put_str(&mut out, &v.acodec);
            out.extend_from_slice(&v.poster.to_le_bytes());
            out.push(v.unreadable as u8 | (v.poster_tried as u8) << 1);
        }
        out
    }

    /// Put the videos saved by [`Library::save_videos`] into this library (whose roots are loaded). Videos of a root that is no longer
    /// in the library are dropped; the id counter moves past every id seen.
    pub fn load_videos(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut r = Rd(bytes);
        if r.take(4)? != VIDEOS_MAGIC || r.u8()? != VIDEOS_VERSION {
            return Err("not a video index".to_string());
        }
        let nroots = r.u16()? as usize;
        let mut remap: Vec<Option<u16>> = Vec::new();
        for _ in 0..nroots {
            let id = r.str()?;
            remap.push(self.roots.iter().position(|x| x.id == id).map(|i| i as u16));
        }
        let n = r.u32()? as usize;
        if n.saturating_mul(30) > r.0.len() {
            return Err("count too large".to_string());
        }
        let mut videos = Vec::with_capacity(n);
        let mut by_id: BTreeMap<u32, ()> = BTreeMap::new();
        for _ in 0..n {
            let id = r.u32()?;
            let root = r.u16()?;
            let path = r.str()?;
            let size = r.u64()?;
            let mtime_ms = r.i64()?;
            let title = r.str()?;
            let duration_us = r.i64()?;
            let (width, height) = (r.u32()?, r.u32()?);
            let (vcodec, acodec) = (r.str()?, r.str()?);
            let poster = r.u64()?;
            let flags = r.u8()?;
            let Some(Some(root)) = remap.get(root as usize).copied() else { continue };
            if by_id.insert(id, ()).is_some() {
                return Err("duplicate video id".to_string());
            }
            self.next_id = self.next_id.max(id + 1);
            videos.push(Video {
                id,
                root,
                path,
                size,
                mtime_ms,
                title,
                duration_us,
                width,
                height,
                vcodec,
                acodec,
                poster,
                poster_tried: flags & 2 != 0,
                unreadable: flags & 1 != 0,
                src: String::new(),
            });
        }
        videos.sort_by_key(|v| v.id);
        self.videos = videos;
        self.videos_dirty = false;
        self.rev += 1;
        Ok(())
    }
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    let mut n = s.len().min(u16::MAX as usize);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    out.extend_from_slice(&(n as u16).to_le_bytes());
    out.extend_from_slice(&s.as_bytes()[..n]);
}

struct Rd<'a>(&'a [u8]);

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.0.len() < n {
            return Err("truncated".to_string());
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().map_err(|_| "x")?))
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| "x")?))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().map_err(|_| "x")?))
    }
    fn i64(&mut self) -> Result<i64, String> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().map_err(|_| "x")?))
    }
    fn str(&mut self) -> Result<String, String> {
        let n = self.u16()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| "not utf-8".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::art::Thumb;
    use crate::model::Root;

    fn entry(path: &str, size: u64, mtime: i64) -> FileEntry {
        FileEntry { id: format!("id:{path}"), path: path.to_string(), size, mtime_ms: mtime }
    }

    fn info(title: &str, dur: i64) -> VideoInfo {
        VideoInfo {
            title: title.into(),
            duration_us: dur,
            width: 1280,
            height: 720,
            vcodec: "av1".into(),
            acodec: "opus".into(),
        }
    }

    fn lib_with(root: &str, files: &[(&str, &str, i64, i64)]) -> Library {
        let mut l = Library::new();
        let list: Vec<FileEntry> = files.iter().map(|f| entry(f.0, 100, f.3)).collect();
        let plan = l.begin_scan(root, "Films", &list);
        for (e, f) in plan.read_videos.iter().zip(files) {
            l.apply_video(plan.root, e, Ok(info(f.1, f.2)));
        }
        l.finish_scan();
        l
    }

    #[test]
    fn orders_by_title_added_and_length() {
        let l = lib_with(
            "r",
            &[("x/b.mp4", "", 30, 300), ("x/a10.mp4", "", 10, 100), ("x/a9.mp4", "Zed", 20, 200)],
        );
        let names = |v: Vec<VideoId>| -> Vec<String> {
            v.into_iter().map(|i| l.video(i).unwrap().display_title().to_string()).collect()
        };
        assert_eq!(names(l.sorted_videos(VideoSort::Title, true)), ["a10", "b", "Zed"]);
        assert_eq!(names(l.sorted_videos(VideoSort::Title, false)), ["Zed", "b", "a10"]);
        assert_eq!(names(l.sorted_videos(VideoSort::Added, true)), ["a10", "Zed", "b"]);
        assert_eq!(names(l.sorted_videos(VideoSort::Length, true)), ["a10", "Zed", "b"]);
        assert_eq!(names(l.sorted_videos(VideoSort::Length, false)), ["b", "Zed", "a10"]);
        assert_eq!(l.total_video_duration_us(), 60);
        assert_eq!(l.search("zed").videos.len(), 1);
    }

    #[test]
    fn natural_title_order_counts_numbers() {
        let l = lib_with("r", &[("a10.mp4", "", 1, 1), ("a9.mp4", "", 1, 2), ("a2.mp4", "", 1, 3)]);
        let order: Vec<&str> = l
            .sorted_videos(VideoSort::Title, true)
            .into_iter()
            .map(|i| l.video(i).unwrap().file_name())
            .collect();
        assert_eq!(order, ["a2.mp4", "a9.mp4", "a10.mp4"]);
    }

    #[test]
    fn saving_and_loading_round_trips_with_posters_and_flags() {
        let mut l = lib_with("r", &[("a.mp4", "A", 5_000_000, 1), ("b.mkv", "", 6_000_000, 2)]);
        let bad = entry("bad.mp4", 1, 1);
        l.apply_video(0, &bad, Err(Error::Invalid("x".into())));
        let (a, b) = (l.all_videos()[0].id, l.all_videos()[1].id);
        l.set_poster(a, Some(Thumb { w: 1, h: 1, rgb: alloc::vec![1, 2, 3] }));
        l.set_poster(b, None);
        assert!(l.videos_dirty());
        let bytes = l.save_videos();
        assert!(!l.videos_dirty());

        let mut m = Library::new();
        m.roots = l.roots.clone();
        m.load_videos(&bytes).unwrap();
        assert_eq!(m.all_videos().len(), 3);
        for (x, y) in l.all_videos().iter().zip(m.all_videos()) {
            let mut x = x.clone();
            x.src.clear();
            assert_eq!(&x, y);
        }
        assert!(m.video(a).unwrap().poster != 0 && m.video(b).unwrap().poster_tried);
        assert!(m.all_videos().iter().any(|v| v.unreadable));
        assert!(!m.videos_dirty());
        assert_eq!(m.video_count(), 2);
        assert!(
            m.used_art().contains(&m.video(a).unwrap().poster),
            "posters are kept when pictures are pruned"
        );
        // New ids continue past the loaded ones.
        let next = m.begin_scan("r", "Films", &[entry("c.mp4", 1, 1)]);
        m.apply_video(next.root, &next.read_videos[0], Ok(info("C", 1)));
        assert!(m.all_videos().iter().find(|v| v.path == "c.mp4").unwrap().id > b);
    }

    #[test]
    fn roots_are_matched_by_id_not_position() {
        let mut l = Library::new();
        l.roots = alloc::vec![
            Root { id: "one".into(), name: "One".into(), connected: true },
            Root { id: "two".into(), name: "Two".into(), connected: true },
        ];
        let p = l.begin_scan("two", "Two", &[entry("t.mp4", 1, 1)]);
        l.apply_video(p.root, &p.read_videos[0], Ok(info("T", 1)));
        let p = l.begin_scan("one", "One", &[entry("o.mp4", 1, 1)]);
        l.apply_video(p.root, &p.read_videos[0], Ok(info("O", 1)));
        let bytes = l.save_videos();
        // The folder list changed: "one" is gone, "two" is now first.
        let mut m = Library::new();
        m.roots = alloc::vec![Root { id: "two".into(), name: "Two".into(), connected: false }];
        m.load_videos(&bytes).unwrap();
        assert_eq!(m.all_videos().len(), 1, "the video of the missing folder is dropped");
        assert_eq!((m.all_videos()[0].path.as_str(), m.all_videos()[0].root), ("t.mp4", 0));
        assert!(m.all_videos()[0].src.is_empty(), "not connected until a listing arrives");
    }

    #[test]
    fn forgetting_a_folder_drops_its_videos() {
        let mut l = lib_with("r", &[("a.mp4", "A", 1, 1)]);
        assert_eq!(l.video_count(), 1);
        l.save_videos();
        l.remove_root("r");
        assert_eq!(l.all_videos().len(), 0);
        assert!(l.videos_dirty());
    }

    #[test]
    fn bad_bytes_are_refused() {
        let mut l = Library::new();
        assert!(l.load_videos(b"").is_err());
        assert!(l.load_videos(b"NOPE\x01").is_err());
        let good = lib_with("r", &[("a.mp4", "A", 1, 1)]).save_videos();
        for cut in [5, 9, good.len() - 1] {
            let mut m = Library::new();
            assert!(m.load_videos(&good[..cut]).is_err(), "cut at {cut}");
        }
    }
}
