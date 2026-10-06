//! The optional write capability: replacing a library file with new bytes (editing its tags).
//!
//! Writing someone's music is the one thing the player does that cannot be undone, so the contract is strict: the host replaces the file
//! **whole and safely** (a temporary file beside it, flushed, then renamed over it; or the browser's own swap file), or it changes
//! nothing and says why. The app never writes a byte it has not first checked reads back as the same audio. A host that cannot write
//! (Rusty Bucket's sandbox, a browser without the File System Access API, a folder added without write permission) says so, and the
//! tag editor opens read-only with that explanation.
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Replacing files.
pub trait FileWriter {
    /// Whether files below `root` (a [`crate::Listing::root`]) can be replaced: `Ok`, or the words that tell the user why not.
    fn can_write(&mut self, root: &str) -> Result<(), String>;
    /// Start replacing `path` (relative to `root`, `/` separated) with `data`; returns a ticket for [`FileWriter::poll_write`]. Must be
    /// called from a user's action (a browser asks for permission then).
    fn write(&mut self, root: &str, path: &str, data: Vec<u8>) -> u32;
    /// The answer for a ticket, once there is one (taken: asking again gives `None`). `Err` carries words for the user; after an `Err`
    /// the file is as it was.
    fn poll_write(&mut self, ticket: u32) -> Option<Result<(), String>>;
}

/// A [`FileWriter`] for tests: it keeps what was written, and can be made to refuse or to fail.
#[derive(Debug, Default)]
pub struct ScriptedWriter {
    /// `Some(reason)`: `can_write` refuses with it.
    pub refuse: Option<String>,
    /// Paths (`root/path`) whose write fails, with the message.
    pub fail: BTreeMap<String, String>,
    /// How many polls answer `None` before the answer comes.
    pub delay_polls: u32,
    /// What was written, in order: `(root, path, bytes)`.
    pub written: Vec<(String, String, Vec<u8>)>,
    waiting: BTreeMap<u32, (Result<(), String>, u32)>,
    next: u32,
}

impl FileWriter for ScriptedWriter {
    fn can_write(&mut self, _root: &str) -> Result<(), String> {
        match &self.refuse {
            Some(r) => Err(r.clone()),
            None => Ok(()),
        }
    }

    fn write(&mut self, root: &str, path: &str, data: Vec<u8>) -> u32 {
        let key = alloc::format!("{root}/{path}");
        let answer = match self.fail.get(&key) {
            Some(m) => Err(m.clone()),
            None => {
                self.written.push((root.into(), path.into(), data));
                Ok(())
            }
        };
        self.next += 1;
        self.waiting.insert(self.next, (answer, self.delay_polls));
        self.next
    }

    fn poll_write(&mut self, ticket: u32) -> Option<Result<(), String>> {
        let (_, left) = self.waiting.get_mut(&ticket)?;
        if *left > 0 {
            *left -= 1;
            return None;
        }
        self.waiting.remove(&ticket).map(|(a, _)| a)
    }
}
