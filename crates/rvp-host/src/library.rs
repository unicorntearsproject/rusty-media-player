//! The optional library capability: directory access for the audio-first view.
//!
//! A host that can show a folder picker and walk a directory tree (a browser with the File System Access API or a
//! `webkitdirectory` input, the desktop, Rusty Bucket's fs) implements [`Library`]. The player asks for work through
//! its effects (`AddFolder`, `Rescan`), the host does it however it likes, and hands the result back as a [`Listing`]
//! the next time the player polls [`Library::take_listing`]. Opening a listed file goes through the ordinary
//! [`crate::Host::open`] with [`FileEntry::id`], so nothing here reads bytes.
use alloc::string::String;
use alloc::vec::Vec;

/// A file found while walking a library root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// What [`crate::OpenRequest::Id`] takes to open it in this session (a path, a stashed file handle).
    pub id: String,
    /// Path relative to the root, `/` separated, with the file name last.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// Modification time, milliseconds since the Unix epoch (0 if the host cannot tell).
    pub mtime_ms: i64,
}

/// A finished walk of one root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    /// Stable id of the root (it survives a reload when the host remembers the folder).
    pub root: String,
    /// Name to show for the root (the folder's name).
    pub name: String,
    /// Every file below the root (not only audio: the library picks what it wants and uses cover images).
    pub files: Vec<FileEntry>,
}

/// Which of the system's standard folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardKind {
    /// The user's Music folder.
    Music,
    /// The user's Videos (Movies) folder.
    Videos,
}

/// One of the folders the operating system keeps a user's music and videos in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardFolder {
    /// Which one.
    pub kind: StandardKind,
    /// The name to show (`Music`).
    pub name: String,
    /// Where it is, in the host's own terms (a path).
    pub path: String,
}

/// Directory access. Optional: hosts without it keep the default (`None`) and the library only has what is opened
/// file by file.
pub trait Library {
    /// A walk the host finished since the last call (after an add-folder or rescan request, or on its own when it
    /// restored a remembered folder).
    fn take_listing(&mut self) -> Option<Listing>;
    /// Roots the host can read right now (a remembered browser folder is not readable again until the user allows it).
    fn connected_roots(&self) -> Vec<String>;
    /// The system's Music and Videos folders that exist (XDG user directories, Windows Known Folders, `~/Music` and `~/Movies`), for the
    /// first run to add. Hosts without such folders (a browser) keep the default: none.
    fn standard_folders(&mut self) -> Vec<StandardFolder> {
        Vec::new()
    }
    /// A sentence for the person when `standard_folders` left one out on purpose (the system names the home folder as the Videos folder: a
    /// library of the whole home would crawl everything), or `None`.
    fn standard_folders_note(&self) -> Option<String> {
        None
    }
    /// Add the folder at `path` to the library without asking: the host walks it and hands the result back through
    /// [`Library::take_listing`] like any other folder. `false` when the host cannot.
    fn add_path(&mut self, _path: &str) -> bool {
        false
    }
}

/// A [`Library`] over lists the test pushes in.
#[derive(Debug, Default)]
pub struct ScriptedLibrary {
    /// Listings to hand out, oldest first.
    pub listings: alloc::collections::VecDeque<Listing>,
    /// What `connected_roots` returns.
    pub connected: Vec<String>,
    /// What `standard_folders` returns.
    pub standard: Vec<StandardFolder>,
    /// The paths `add_path` was asked for.
    pub added: Vec<String>,
    /// What `standard_folders_note` returns.
    pub note: Option<String>,
}

impl Library for ScriptedLibrary {
    fn take_listing(&mut self) -> Option<Listing> {
        self.listings.pop_front()
    }

    fn connected_roots(&self) -> Vec<String> {
        self.connected.clone()
    }

    fn standard_folders(&mut self) -> Vec<StandardFolder> {
        self.standard.clone()
    }

    fn add_path(&mut self, path: &str) -> bool {
        self.added.push(path.into());
        true
    }

    fn standard_folders_note(&self) -> Option<String> {
        self.note.clone()
    }
}
