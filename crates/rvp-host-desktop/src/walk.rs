//! Walking a music folder on a thread of its own (a big library must not freeze the window) and handing the result to the app as a
//! [`Listing`].
use rvp_host::{FileEntry, Listing};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png"];
const LIST_EXTENSIONS: &[&str] = &["m3u", "m3u8", "pls"];
/// Directories are followed this deep (a symlink loop must end, and no real library is deeper).
const MAX_DEPTH: usize = 24;
/// A walk stops after this many wanted files (a sanity bound: 20 times a very large music library) and says so.
const MAX_FILES: usize = 500_000;
/// ... and after this many directories.
const MAX_DIRS: usize = 200_000;
/// Directories that are never music or films and can be huge: build output, package caches, trash, the file system's own.
const JUNK_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "__pycache__",
    "lost+found",
    "$RECYCLE.BIN",
    "System Volume Information",
    "site-packages",
    "venv",
    "vendor",
];

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

/// Whether a directory is never worth walking: hidden, or one of the known junk names.
fn skip_dir(name: &str) -> bool {
    name.starts_with('.') || JUNK_DIRS.iter().any(|j| j.eq_ignore_ascii_case(name))
}

/// What identifies a directory beyond its path: two paths that reach the same directory (a symlink loop, a bind mount) have the same one.
#[cfg(unix)]
fn dir_identity(md: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (md.dev(), md.ino())
}
#[cfg(not(unix))]
fn dir_identity(_md: &std::fs::Metadata) -> (u64, u64) {
    (0, 0)
}

/// Every file of interest below `dir`, sorted by path. Hidden directories and known junk are skipped, a directory is walked once however
/// many links reach it (so a loop cannot make the walk run away), a symlinked folder or file is followed, files are told by their extension
/// before they are looked at (a folder with a million other files costs one directory read, not a million stats), and the walk is bounded in
/// depth, directories and files.
pub fn list_dir(dir: &Path) -> Listing {
    struct Walk {
        seen: std::collections::HashSet<(u64, u64)>,
        files: Vec<FileEntry>,
        dirs: usize,
    }
    fn walk(base: &Path, dir: &Path, depth: usize, w: &mut Walk) {
        if depth > MAX_DEPTH || w.dirs >= MAX_DIRS || w.files.len() >= MAX_FILES {
            return;
        }
        let Ok(dmd) = std::fs::metadata(dir) else { return };
        // `(0, 0)` where there is no identity (not unix): the depth cap alone ends a loop there.
        let id = dir_identity(&dmd);
        if id != (0, 0) && !w.seen.insert(id) {
            return;
        }
        w.dirs += 1;
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            if w.files.len() >= MAX_FILES {
                return;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let p = e.path();
            // The kind without a stat where the directory read already says it; a symlink is resolved (it may be a folder or a file).
            let Ok(ft) = e.file_type() else { continue };
            let (is_dir, is_file) = if ft.is_symlink() {
                match std::fs::metadata(&p) {
                    Ok(m) => (m.is_dir(), m.is_file()),
                    Err(_) => continue, // a broken link
                }
            } else {
                (ft.is_dir(), ft.is_file())
            };
            if is_dir {
                if !skip_dir(&name) {
                    walk(base, &p, depth + 1, w);
                }
            } else if is_file && wanted(&name) {
                let Ok(md) = std::fs::metadata(&p) else { continue };
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
                w.files.push(FileEntry {
                    id: p.to_string_lossy().into_owned(),
                    path: rel,
                    size: md.len(),
                    mtime_ms,
                });
            }
        }
    }
    let mut w = Walk { seen: Default::default(), files: Vec::new(), dirs: 0 };
    walk(dir, dir, 0, &mut w);
    let name = dir
        .file_name()
        .map_or_else(|| dir.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
    Listing { root: root_id(dir), name, files: w.files }
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

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rvp-walk-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_folder_is_followed_and_a_symlink_loop_ends() {
        use std::os::unix::fs::symlink;
        let real = scratch("real");
        let base = scratch("lib");
        std::fs::create_dir_all(real.join("Artist")).unwrap();
        std::fs::write(real.join("Artist/a.flac"), b"x").unwrap();
        // Music is a link to the real folder (valid); inside the real folder a link points back up to the library (a loop), another
        // to itself, and one is broken.
        symlink(&real, base.join("Music")).unwrap();
        symlink(&base, real.join("Artist/back-to-library")).unwrap();
        symlink(real.join("Artist"), real.join("Artist/self")).unwrap();
        symlink(real.join("nowhere"), real.join("Artist/broken")).unwrap();
        let l = list_dir(&base);
        let paths: Vec<_> = l.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["Music/Artist/a.flac"], "found once, loops and broken links ignored");
        // A link to a single file is followed too.
        symlink(real.join("Artist/a.flac"), base.join("same.flac")).unwrap();
        assert_eq!(list_dir(&base).files.len(), 2);
    }

    #[test]
    fn junk_and_non_media_are_skipped_without_looking_at_them() {
        let base = scratch("junk");
        for d in ["node_modules/pkg", "target/debug", "Artist/Album", "__pycache__", ".git/objects"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        for f in [
            "node_modules/pkg/x.mp3",
            "target/debug/y.flac",
            "__pycache__/z.ogg",
            ".git/objects/w.mp3",
            "Artist/Album/keep.mp3",
            "Artist/Album/big.iso",
            "Artist/Album/notes.docx",
        ] {
            std::fs::write(base.join(f), b"x").unwrap();
        }
        let l = list_dir(&base);
        let paths: Vec<_> = l.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["Artist/Album/keep.mp3"]);
    }

    #[test]
    fn a_deep_tree_stops_at_the_depth_cap() {
        let mut d = scratch("deep");
        let base = d.clone();
        for i in 0..(MAX_DEPTH + 6) {
            d = d.join(format!("d{i}"));
        }
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("too-deep.mp3"), b"x").unwrap();
        std::fs::write(base.join("d0/shallow.mp3"), b"x").unwrap();
        let l = list_dir(&base);
        let paths: Vec<_> = l.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["d0/shallow.mp3"]);
    }
}
