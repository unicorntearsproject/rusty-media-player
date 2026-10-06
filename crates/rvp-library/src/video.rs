//! Videos in the library: the files, what the container says about them, and a poster picture made by decoding one frame.
//!
//! They live next to the music: the same folders are scanned, the same index file names them (a separate `library/videos` value, see
//! [`crate::persist`]), the same thumbnail cache holds their posters and the same queue plays them. A video has no albums or artists; it is
//! listed by title, by when it was added, or by length.
use crate::art::{self, Image, Thumb};
use crate::fold::hash64;
use crate::model::ArtId;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use rvp_core::task::yield_now;
use rvp_core::{CodecFactory, Error, StreamKind, color::yuv420_to_rgba};
use rvp_demux::Demuxer;
use rvp_host::Source;

/// Id of a video: from the same counter as the tracks' ids, so the queue can tell them apart by looking either up.
pub type VideoId = u32;

/// File name extensions of the videos the library indexes.
pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "m4v", "mkv", "webm"];

/// Longest side of a poster, pixels (a 16:9 frame is 320 x 180).
pub const POSTER_SIDE: u32 = 320;

/// True for a file the library indexes as a video.
pub fn is_video_name(name: &str) -> bool {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.as_str()))
}

/// One video file.
#[derive(Debug, Clone, PartialEq)]
pub struct Video {
    /// Stable id.
    pub id: VideoId,
    /// Index into [`crate::Library::roots`].
    pub root: u16,
    /// Path below the root, `/` separated.
    pub path: String,
    /// File size when it was read.
    pub size: u64,
    /// Modification time when it was read, milliseconds since the Unix epoch.
    pub mtime_ms: i64,
    /// Title tag (empty if missing).
    pub title: String,
    /// Length, microseconds (0 if unknown).
    pub duration_us: i64,
    /// Picture width, pixels (0 if unknown).
    pub width: u32,
    /// Picture height, pixels.
    pub height: u32,
    /// Video codec name (`h264`, `av1`, `vp9`).
    pub vcodec: String,
    /// Codec of the first audio stream (empty when there is none).
    pub acodec: String,
    /// The poster picture (a frame of the film); 0 until it has been made.
    pub poster: ArtId,
    /// A poster was tried for and could not be made (not tried again until the file changes).
    pub poster_tried: bool,
    /// The file could not be read as video; it is remembered (so it is not read again) but not shown.
    pub unreadable: bool,
    /// What the host opens this file with in this session; empty when its root is not connected.
    pub src: String,
}

impl Video {
    /// The file name without its directory.
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// The directory below the root (empty at the top).
    pub fn dir(&self) -> &str {
        self.path.rsplit_once('/').map_or("", |(d, _)| d)
    }

    /// The title to show: the tag, or the file name without its extension.
    pub fn display_title(&self) -> &str {
        if !self.title.is_empty() {
            return &self.title;
        }
        let f = self.file_name();
        match f.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() && ext.len() <= 5 => stem,
            _ => f,
        }
    }

    /// `1280x720`, or empty when the size is not known.
    pub fn size_text(&self) -> String {
        if self.width == 0 || self.height == 0 {
            String::new()
        } else {
            alloc::format!("{}x{}", self.width, self.height)
        }
    }
}

/// What the container says about a video file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VideoInfo {
    /// Title tag.
    pub title: String,
    /// Length, microseconds.
    pub duration_us: i64,
    /// Picture width.
    pub width: u32,
    /// Picture height.
    pub height: u32,
    /// Video codec.
    pub vcodec: String,
    /// Audio codec of the first audio stream.
    pub acodec: String,
}

/// Read the headers of a video file: length, picture size, codecs and the title tag.
pub async fn read_video_info<S: Source>(src: S) -> Result<VideoInfo, Error> {
    let d = rvp_demux::open(src).await?;
    let video = d
        .streams()
        .iter()
        .find(|s| s.kind == StreamKind::Video)
        .ok_or_else(|| Error::Unsupported("no video stream".to_string()))?
        .clone();
    let audio = d.streams().iter().find(|s| s.kind == StreamKind::Audio);
    let m = d.metadata();
    Ok(VideoInfo {
        title: m.title.clone().unwrap_or_default(),
        duration_us: d.duration_us().or(video.duration_us).unwrap_or(0).max(0),
        width: video.video.map_or(0, |v| v.width),
        height: video.video.map_or(0, |v| v.height),
        vcodec: video.codec.clone(),
        acodec: audio.map(|a| a.codec.clone()).unwrap_or_default(),
    })
}

/// How far into the film the poster frame is taken: a tenth of it, but not before the first second and not after the first minute (most
/// films open on a title card or black; a sample from just after is usually a face or a place).
pub fn poster_time_us(duration_us: i64) -> i64 {
    if duration_us <= 0 {
        return 0;
    }
    let tenth = duration_us / 10;
    let t = tenth.clamp(1_000_000, 60_000_000);
    // A very short clip: the middle.
    if duration_us < 4_000_000 { duration_us / 2 } else { t.min(duration_us - 1_000_000) }
}

/// The id of a video's poster picture: of the file (root, path, time and size), so a changed file gets a new one.
pub fn poster_id(root: u16, path: &str, mtime_ms: i64, size: u64) -> ArtId {
    let mut b = Vec::new();
    b.extend_from_slice(&root.to_le_bytes());
    b.extend_from_slice(path.as_bytes());
    b.extend_from_slice(&mtime_ms.to_le_bytes());
    b.extend_from_slice(&size.to_le_bytes());
    b.extend_from_slice(b"poster");
    hash64(&b).max(2)
}

/// Decode one frame of the film (near [`poster_time_us`]) and return it as a poster thumbnail. `Ok(None)` when no frame came out of the
/// first few hundred packets (a stream the decoders cannot read, a file that is all audio).
pub async fn make_poster<S: Source>(src: S, codecs: &dyn CodecFactory) -> Result<Option<Thumb>, Error> {
    let mut d = rvp_demux::open(src).await?;
    let info = d
        .streams()
        .iter()
        .find(|s| s.kind == StreamKind::Video)
        .ok_or_else(|| Error::Unsupported("no video stream".to_string()))?
        .clone();
    let at = poster_time_us(d.duration_us().or(info.duration_us).unwrap_or(0));
    if at > 0 {
        d.seek(at).await?;
    }
    let mut dec = codecs.video(&info)?;
    let mut sent = 0usize;
    while let Some(p) = d.next_packet().await? {
        if p.stream_id != info.id {
            continue;
        }
        sent += 1;
        if sent > MAX_PACKETS {
            return Ok(None);
        }
        if sent % 16 == 0 {
            yield_now().await;
        }
        if dec.send_packet(&p).is_err() {
            continue;
        }
        // A decoder on its own thread answers a little later: poll until it has nothing pending.
        let mut spins = 0;
        loop {
            match dec.receive_frame() {
                Ok(Some(f)) => return Ok(Some(frame_to_poster(&f))),
                Ok(None) if dec.pending() > 0 && spins < 2000 => {
                    spins += 1;
                    yield_now().await;
                }
                _ => break,
            }
        }
    }
    // The end of the stream: a decoder that holds frames back gives them up now.
    let _ = dec.drain();
    while let Ok(Some(f)) = dec.receive_frame() {
        return Ok(Some(frame_to_poster(&f)));
    }
    Ok(None)
}

/// Packets sent to the decoder before a poster is given up on.
const MAX_PACKETS: usize = 600;

fn frame_to_poster(f: &rvp_core::VideoFrame) -> Thumb {
    let mut rgba = alloc::vec![0u8; f.width as usize * f.height as usize * 4];
    yuv420_to_rgba(f, &mut rgba);
    let img = Image { w: f.width, h: f.height, rgba };
    let small = art::downscale(&img, POSTER_SIDE);
    let mut rgb = Vec::with_capacity((small.w * small.h * 3) as usize);
    for p in small.rgba.chunks_exact(4) {
        rgb.extend_from_slice(&p[..3]);
    }
    Thumb { w: small.w as u16, h: small.h as u16, rgb }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_told_apart() {
        assert!(
            is_video_name("Film.MKV")
                && is_video_name("a.b.mp4")
                && is_video_name("x.webm")
                && is_video_name("y.m4v")
        );
        assert!(
            !is_video_name("song.mp3")
                && !is_video_name("cover.jpg")
                && !is_video_name("noext")
                && !is_video_name("a.mp4.txt")
        );
    }

    #[test]
    fn titles_fall_back_to_the_file_name() {
        let mut v = Video {
            id: 1,
            root: 0,
            path: "Films/Heat (1995).mkv".into(),
            size: 1,
            mtime_ms: 1,
            title: String::new(),
            duration_us: 0,
            width: 1920,
            height: 1080,
            vcodec: "h264".into(),
            acodec: "aac".into(),
            poster: 0,
            poster_tried: false,
            unreadable: false,
            src: String::new(),
        };
        assert_eq!(v.display_title(), "Heat (1995)");
        assert_eq!(v.dir(), "Films");
        assert_eq!(v.size_text(), "1920x1080");
        v.title = "Heat".into();
        assert_eq!(v.display_title(), "Heat");
        v.width = 0;
        assert_eq!(v.size_text(), "");
    }

    #[test]
    fn the_poster_frame_is_a_tenth_in_within_limits() {
        assert_eq!(poster_time_us(0), 0);
        assert_eq!(poster_time_us(3_000_000), 1_500_000, "a short clip: the middle");
        assert_eq!(poster_time_us(20_000_000), 2_000_000);
        assert_eq!(poster_time_us(5_000_000), 1_000_000, "not before the first second");
        assert_eq!(poster_time_us(2 * 3600 * 1_000_000), 60_000_000, "not after the first minute");
        assert!(poster_time_us(5_000_000) <= 4_000_000);
    }

    #[test]
    fn poster_ids_follow_the_file() {
        let a = poster_id(0, "a.mp4", 100, 5);
        assert_eq!(a, poster_id(0, "a.mp4", 100, 5));
        assert_ne!(a, poster_id(0, "a.mp4", 101, 5));
        assert_ne!(a, poster_id(1, "a.mp4", 100, 5));
        assert!(a >= 2, "never the 'none' or the fallback id");
    }
}
