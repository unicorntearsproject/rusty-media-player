//! Native file dialogs (the desktop portal on Linux, the common dialogs on Windows) on a worker thread, so the window and the audio
//! feed keep running while one is open. Results come back through a channel the event loop polls.
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

/// Extensions the open dialog offers.
pub const MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "mka", "mp3", "mp2", "flac", "ogg", "oga", "opus", "wav", "m4a", "m4b", "aac",
    "srt", "vtt", "m3u", "m3u8", "pls",
];

/// What a dialog produced.
#[derive(Debug)]
pub enum Picked {
    /// Files to open (`append`: add to the queue instead of replacing it).
    Files {
        /// The chosen files.
        files: Vec<PathBuf>,
        /// Add rather than replace.
        append: bool,
    },
    /// A library folder.
    Folder(PathBuf),
    /// An exported file was written (or not): a message for the user.
    Saved(String),
}

/// Starts dialogs and collects what they return.
pub struct Dialogs {
    tx: Sender<Picked>,
    rx: Receiver<Picked>,
    busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

struct BusyGuard(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::Release);
    }
}

impl Default for Dialogs {
    fn default() -> Self {
        Self::new()
    }
}

impl Dialogs {
    /// No dialog open.
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self { tx, rx, busy: Default::default() }
    }

    /// The next result, if one is ready.
    pub fn poll(&self) -> Option<Picked> {
        self.rx.try_recv().ok()
    }

    fn spawn(&self, f: impl FnOnce(Sender<Picked>) + Send + 'static) {
        use std::sync::atomic::Ordering;
        if self.busy.swap(true, Ordering::AcqRel) {
            return; // one dialog at a time
        }
        let guard = BusyGuard(self.busy.clone());
        let tx = self.tx.clone();
        let _ = std::thread::Builder::new().name("rvp-dialog".into()).spawn(move || {
            let _guard = guard;
            f(tx);
        });
    }

    /// "Open files".
    pub fn pick_files(&self, append: bool) {
        self.spawn(move |tx| {
            let files = rfd::FileDialog::new()
                .set_title(if append { "Add to the queue" } else { "Open" })
                .add_filter("Media, subtitles and playlists", MEDIA_EXTENSIONS)
                .add_filter("All files", &["*"])
                .pick_files();
            if let Some(files) = files {
                let _ = tx.send(Picked::Files { files, append });
            }
        });
    }

    /// "Add a folder to the library".
    pub fn pick_folder(&self) {
        self.spawn(move |tx| {
            if let Some(dir) = rfd::FileDialog::new().set_title("Add a folder to the library").pick_folder() {
                let _ = tx.send(Picked::Folder(dir));
            }
        });
    }

    /// "Import a playlist".
    pub fn pick_playlists(&self) {
        self.spawn(move |tx| {
            let files = rfd::FileDialog::new()
                .set_title("Import a playlist")
                .add_filter("Playlists", &["m3u", "m3u8", "pls"])
                .pick_files();
            if let Some(files) = files {
                let _ = tx.send(Picked::Files { files, append: true });
            }
        });
    }

    /// "Save a playlist".
    pub fn save(&self, name: String, data: Vec<u8>) {
        self.spawn(move |tx| {
            let ext = name.rsplit_once('.').map_or("m3u8", |(_, e)| e).to_string();
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Save")
                .set_file_name(&name)
                .add_filter("Playlist", &[ext.as_str()])
                .save_file()
            {
                let msg = match std::fs::write(&path, data) {
                    Ok(()) => format!(
                        "Saved {}",
                        path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())
                    ),
                    Err(e) => format!("Couldn't save: {e}"),
                };
                let _ = tx.send(Picked::Saved(msg));
            }
        });
    }
}
