//! Fetching a page of text for the theme dialog: one HTTPS GET on a worker thread, answered by ticket. It uses the update client, so it never
//! leaves HTTPS, follows at most four redirects, times out, and never reads more than [`rvp_host::MAX_FETCH_BYTES`].
use rvp_host::{MAX_FETCH_BYTES, Net};
use rvp_update::fetch::fetch_bytes;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

/// The desktop's [`Net`].
pub struct DesktopNet {
    agent: Arc<rvp_update::Net>,
    next: u32,
    pending: HashMap<u32, Receiver<Result<String, String>>>,
}

impl Default for DesktopNet {
    fn default() -> Self {
        Self::new()
    }
}

impl DesktopNet {
    /// A client that says who it is.
    pub fn new() -> Self {
        let ua = format!("RustyWave/{}", env!("CARGO_PKG_VERSION"));
        Self { agent: Arc::new(rvp_update::Net::new(&ua)), next: 0, pending: HashMap::new() }
    }
}

impl Net for DesktopNet {
    fn fetch_text(&mut self, url: &str) -> u32 {
        self.next += 1;
        let (tx, rx) = channel();
        self.pending.insert(self.next, rx);
        let (agent, url) = (self.agent.clone(), url.to_string());
        std::thread::spawn(move || {
            let r = if !url.starts_with("https://") {
                Err("only https links are fetched".to_string())
            } else {
                fetch_bytes(&*agent, &url, MAX_FETCH_BYTES as u64)
                    .map_err(|e| e.to_string())
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
            };
            let _ = tx.send(r);
        });
        self.next
    }

    fn poll_fetch(&mut self, id: u32) -> Option<Result<String, String>> {
        let r = self.pending.get(&id)?.try_recv();
        match r {
            Ok(v) => {
                self.pending.remove(&id);
                Some(v)
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pending.remove(&id);
                Some(Err("the fetch stopped unexpectedly".into()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_http_is_refused_in_words_and_tickets_are_taken_once() {
        let mut n = DesktopNet::new();
        let id = n.fetch_text("http://example.com/x.css");
        let mut got = None;
        for _ in 0..200 {
            got = n.poll_fetch(id);
            if got.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(got, Some(Err("only https links are fetched".to_string())));
        assert_eq!(n.poll_fetch(id), None, "taken");
        assert_eq!(n.poll_fetch(999), None);
    }
}
