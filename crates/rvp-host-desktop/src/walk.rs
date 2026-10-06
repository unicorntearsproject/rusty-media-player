//! Walking a music folder on a thread of its own (a big library must not freeze the window) and handing the result to the app as a
//! [`Listing`].
use rvp_host::{FileEntry, Listing};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png"];
const LIST_EXTENSIONS: &[&str] = &["m3u", "m3u8", "pls"];
/// Directories are followed this deep (a symlink loop must end).
const MAX_DEPTH: usize = 40;

/// The root id of a folder: stable across runs because it is the path.
pub fn root_id(dir: &Path) -> String {
    format!("dir:{}", dir.to_string_lossy())
}

/// The folder a root id names, if it is one of ours.
pub fn root_path(id: &str) -> Option<PathBuf> {
    id.strip_prefix("dir:").map(PathBuf::from)
}

fn wanted(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext {
        Some(e) => {
            rvp_library::AUDIO_EXTENSIONS.contains(&e.as_str())
                || rvp_library::VIDEO_EXTENSIONS.contains(&e.as_str())
                || IMAGE_EXTENSIONS.contains(&e.as_str())
                || LIST_EXTENSIONS.contains(&e.as_str())
        }
        None => false,
    }
}

/// Every file of interest below `dir`, sorted by path. Hidden directories are skipped.
pub fn list_dir(dir: &Path) -> Listing {
    fn walk(base: &Path, dir: &Path, depth: usize, out: &mut Vec<FileEntry>) {
        if depth > MAX_DEPTH {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let p = e.path();
            let Ok(md) = std::fs::metadata(&p) else { continue };
            if md.is_dir() {
                if !name.starts_with('.') {
                    walk(base, &p, depth + 1, out);
                }
            } else if md.is_file() && wanted(&name) {
                let rel = p
                    .strip_prefix(base)
                    .unwrap_or(&p)
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                let mtime_ms = md
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_millis() as i64);
                out.push(FileEntry {
                    id: p.to_string_lossy().into_owned(),
                    path: rel,
                    size: md.len(),
                    mtime_ms,
                });
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, dir, 0, &mut files);
    let name = dir
        .file_name()
        .map_or_else(|| dir.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
    Listing { root: root_id(dir), name, files }
}

/// Walk `dir` on a new thread and send the listing.
pub fn spawn_walk(dir: PathBuf, tx: Sender<Listing>) {
    let _ = std::thread::Builder::new().name("rvp-walk".into()).spawn(move || {
        let listing = list_dir(&dir);
        let _ = tx.send(listing);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_music_and_covers_below_a_folder() {
        let base = std::env::temp_dir().join(format!("rvp-walk-{}", std::process::id()));
        let album = base.join("Artist").join("Album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::create_dir_all(base.join(".hidden")).unwrap();
        std::fs::create_dir_all(base.join("Films")).unwrap();
        for f in [
            "Artist/Album/01 a.mp3",
            "Artist/Album/cover.JPG",
            "Artist/Album/notes.txt",
            ".hidden/x.mp3",
            "root.flac",
            "list.m3u8",
            "Films/night.webm",
            "Films/day.MP4",
        ] {
            std::fs::write(base.join(f), b"x").unwrap();
        }
        let l = list_dir(&base);
        let paths: Vec<_> = l.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "Artist/Album/01 a.mp3",
                "Artist/Album/cover.JPG",
                "Films/day.MP4",
                "Films/night.webm",
                "list.m3u8",
                "root.flac"
            ]
        );
        assert!(l.root.starts_with("dir:") && root_path(&l.root).unwrap() == base);
        assert!(l.files[0].id.ends_with("01 a.mp3") && l.files[0].size == 1);
        for f in [
            "Artist/Album/01 a.mp3",
            "Artist/Album/cover.JPG",
            "Artist/Album/notes.txt",
            ".hidden/x.mp3",
            "root.flac",
            "list.m3u8",
        ] {
            std::fs::remove_file(base.join(f)).unwrap();
        }
        for d in ["Artist/Album", "Artist", ".hidden", ""] {
            std::fs::remove_dir(base.join(d)).ok();
        }
    }
}
