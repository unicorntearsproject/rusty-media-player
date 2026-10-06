//! The optional network capability: fetching a page of text the user pointed the app at (a link to a design system's style sheet).
//!
//! Deliberately tiny: one GET of a text document, polled, bounded in size. The app never sends anything but the address, and only when the
//! user pressed *Preview* in the theme dialog. A host that cannot reach the network, or has no business doing so (Rusty Bucket's sandbox,
//! a page whose browser refuses a cross-origin read), keeps the default (`None`) or answers with an error, and the app says to paste the
//! text instead.
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// The most text a fetch may return (a style sheet, a page).
pub const MAX_FETCH_BYTES: usize = 2 << 20;

/// Fetching text over HTTP(S).
pub trait Net {
    /// Start fetching `url`; returns a ticket for [`Net::poll_fetch`]. A host refuses anything that is not `http(s)` by answering with an
    /// error, not by panicking.
    fn fetch_text(&mut self, url: &str) -> u32;
    /// The answer for a ticket, once there is one (taken: asking again gives `None`). `Err` carries words for the user.
    fn poll_fetch(&mut self, id: u32) -> Option<Result<String, String>>;
}

/// A [`Net`] for tests: addresses map to answers, and every request is recorded.
#[derive(Debug, Default)]
pub struct ScriptedNet {
    /// What each address answers (an address that is not here answers `Err("not found")`).
    pub pages: BTreeMap<String, Result<String, String>>,
    /// The addresses asked for, in order.
    pub requests: Vec<String>,
    /// How many polls answer `None` before the answer comes (a slow network).
    pub delay_polls: u32,
    waiting: BTreeMap<u32, (String, u32)>,
    next: u32,
}

impl Net for ScriptedNet {
    fn fetch_text(&mut self, url: &str) -> u32 {
        self.requests.push(url.into());
        self.next += 1;
        self.waiting.insert(self.next, (url.into(), self.delay_polls));
        self.next
    }

    fn poll_fetch(&mut self, id: u32) -> Option<Result<String, String>> {
        let (url, left) = self.waiting.get_mut(&id)?;
        if *left > 0 {
            *left -= 1;
            return None;
        }
        let url = url.clone();
        self.waiting.remove(&id);
        Some(self.pages.get(&url).cloned().unwrap_or_else(|| Err("not found".into())))
    }
}
