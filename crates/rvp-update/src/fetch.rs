//! Getting bytes: HTTPS in production, local files in tests (and for `RVP_UPDATE_MANIFEST_URL=file://...`).
use crate::error::UpdateError;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// A flag the user's Cancel sets and a download polls.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    /// A fresh, unset flag.
    pub fn new() -> Cancel {
        Cancel::default()
    }
    /// Ask for the work to stop.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    /// True once cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Where bytes come from.
pub trait Source: Send + Sync {
    /// Open `url`: a reader and the length the server announced, if it did.
    fn open(&self, url: &str) -> Result<(Box<dyn Read + Send>, Option<u64>), UpdateError>;
}

/// HTTPS (only) and local files.
pub struct Net {
    agent: ureq::Agent,
}

/// The User-Agent of the app's requests: `RustyWave/<version>`. For tests and acceptance runs, `RVP_UPDATE_UA_PREFIX` (say
/// `rw-acceptance/1`) is put in front, so the release site can tell automated requests (`rw-` ...) from people's.
pub fn user_agent(version: &str) -> String {
    match std::env::var("RVP_UPDATE_UA_PREFIX") {
        Ok(p) if !p.trim().is_empty() => format!("{} RustyWave/{version}", p.trim()),
        _ => format!("RustyWave/{version}"),
    }
}

impl Net {
    /// A client that identifies itself as `user_agent` and never leaves HTTPS.
    pub fn new(user_agent: &str) -> Net {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .https_only(true)
            .user_agent(user_agent)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_recv_body(Some(Duration::from_secs(30)))
            .max_redirects(4)
            .build()
            .into();
        Net { agent }
    }
}

/// The path a `file://` URL or a plain path names.
fn local_path(url: &str) -> Option<PathBuf> {
    if let Some(rest) = url.strip_prefix("file://") {
        // file:///abs/path, and on Windows file:///C:/x
        let p = if rest.len() > 2 && rest.as_bytes()[0] == b'/' && rest.as_bytes()[2] == b':' {
            &rest[1..]
        } else {
            rest
        };
        return Some(PathBuf::from(p));
    }
    if url.contains("://") { None } else { Some(PathBuf::from(url)) }
}

impl Source for Net {
    fn open(&self, url: &str) -> Result<(Box<dyn Read + Send>, Option<u64>), UpdateError> {
        if let Some(p) = local_path(url) {
            let f = std::fs::File::open(&p).map_err(|e| UpdateError::Io(format!("{}: {e}", p.display())))?;
            let len = f.metadata().ok().map(|m| m.len());
            return Ok((Box::new(f), len));
        }
        if !url.starts_with("https://") {
            return Err(UpdateError::Insecure(url.to_string()));
        }
        match self.agent.get(url).call() {
            Ok(resp) => {
                let len = resp.body().content_length();
                Ok((Box::new(resp.into_body().into_reader()), len))
            }
            Err(ureq::Error::StatusCode(c)) => Err(UpdateError::Http(c)),
            Err(e) => Err(UpdateError::Io(format!("cannot reach {url}: {e}"))),
        }
    }
}

/// Read all of `url` (at most `max` bytes).
pub fn fetch_bytes(src: &dyn Source, url: &str, max: u64) -> Result<Vec<u8>, UpdateError> {
    let (mut r, len) = src.open(url)?;
    if len.is_some_and(|l| l > max) {
        return Err(UpdateError::TooLarge);
    }
    let mut out = Vec::new();
    r.by_ref().take(max + 1).read_to_end(&mut out)?;
    if out.len() as u64 > max {
        return Err(UpdateError::TooLarge);
    }
    Ok(out)
}

/// What a finished download is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Downloaded {
    /// Bytes written.
    pub size: u64,
    /// Lower-case hex SHA-256 of them.
    pub sha256: String,
}

/// Download `url` into `dest`, at most `max` bytes, calling `progress(done, total)` as it goes, stopping when `cancel` is set. The file is
/// left in place on success and removed on any failure.
pub fn download_to(
    src: &dyn Source,
    url: &str,
    dest: &Path,
    max: u64,
    cancel: &Cancel,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Downloaded, UpdateError> {
    let result = download_inner(src, url, dest, max, cancel, progress);
    if result.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    result
}

fn download_inner(
    src: &dyn Source,
    url: &str,
    dest: &Path,
    max: u64,
    cancel: &Cancel,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Downloaded, UpdateError> {
    let (mut r, len) = src.open(url)?;
    let total = len.unwrap_or(max);
    if len.is_some_and(|l| l > max) {
        return Err(UpdateError::TooLarge);
    }
    let mut f = std::fs::File::create(dest)?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    progress(0, total);
    loop {
        if cancel.is_cancelled() {
            return Err(UpdateError::Cancelled);
        }
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        done += n as u64;
        if done > max {
            return Err(UpdateError::TooLarge);
        }
        hash.update(&buf[..n]);
        f.write_all(&buf[..n])?;
        progress(done, total.max(done));
    }
    f.flush()?;
    f.sync_all().ok();
    Ok(Downloaded { size: done, sha256: hex(&hash.finalize()) })
}

/// Lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 of `data`, hex.
pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rvp-update-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn local_paths_and_https_only() {
        let net = Net::new("test");
        let d = tmp("fetch1");
        std::fs::write(d.join("a"), b"hello").unwrap();
        let url = format!("file://{}", d.join("a").display());
        assert_eq!(fetch_bytes(&net, &url, 100).unwrap(), b"hello");
        assert_eq!(fetch_bytes(&net, d.join("a").to_str().unwrap(), 100).unwrap(), b"hello");
        assert_eq!(fetch_bytes(&net, &url, 4), Err(UpdateError::TooLarge));
        assert!(matches!(fetch_bytes(&net, "http://example.com/x", 10), Err(UpdateError::Insecure(_))));
        assert!(matches!(fetch_bytes(&net, "ftp://example.com/x", 10), Err(UpdateError::Insecure(_))));
        assert!(matches!(
            fetch_bytes(&net, &format!("file://{}", d.join("missing").display()), 10),
            Err(UpdateError::Io(_))
        ));
    }

    #[test]
    fn download_hashes_reports_progress_and_cleans_up() {
        let net = Net::new("test");
        let d = tmp("fetch2");
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(d.join("src"), &data).unwrap();
        let src = d.join("src").to_string_lossy().into_owned();
        let dest = d.join("dest");
        let mut seen = Vec::new();
        let got =
            download_to(&net, &src, &dest, 1 << 20, &Cancel::new(), &mut |a, b| seen.push((a, b))).unwrap();
        assert_eq!(got.size, 300_000);
        assert_eq!(got.sha256, sha256_hex(&data));
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert_eq!(seen.first(), Some(&(0, 300_000)));
        assert_eq!(seen.last(), Some(&(300_000, 300_000)));
        assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0));
        // Over the limit: refused and nothing left behind.
        let small = d.join("small");
        assert_eq!(
            download_to(&net, &src, &small, 1000, &Cancel::new(), &mut |_, _| {}),
            Err(UpdateError::TooLarge)
        );
        assert!(!small.exists());
        // Cancelled midway: nothing left behind.
        let c = Cancel::new();
        let c2 = c.clone();
        let part = d.join("part");
        let r = download_to(&net, &src, &part, 1 << 20, &c, &mut |done, _| {
            if done > 100_000 {
                c2.cancel()
            }
        });
        assert_eq!(r, Err(UpdateError::Cancelled));
        assert!(!part.exists());
    }
}
