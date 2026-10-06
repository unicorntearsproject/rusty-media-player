//! Opening a web page in the person's own browser (the About page's links).
use std::process::{Command, Stdio};

/// True for an address that is safe to hand to the system: `https://` and a host, no spaces or control characters.
pub fn is_web_url(url: &str) -> bool {
    url.strip_prefix("https://").is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
        && url.chars().all(|c| !c.is_whitespace() && !c.is_control())
}

/// Open `url` with the system's default browser. Anything that is not a plain `https` address is ignored, and so is a failure
/// (there is nobody to tell: the About page also shows the address).
pub fn open_url(url: &str) {
    if !is_web_url(url) {
        return;
    }
    #[cfg(target_os = "windows")]
    let mut cmd = {
        // `rundll32` takes the address as one argument; `cmd /c start` would read `&` in it as a command separator.
        let mut c = Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler").arg(url);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    // Leave the browser running when the player quits; do not wait for it.
    if let Ok(mut child) = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_https_addresses_are_opened() {
        assert!(is_web_url("https://rustybucket.ai"));
        assert!(is_web_url("https://x.com/djunicorntears"));
        assert!(is_web_url("https://example.com/a?b=1&c=2"));
        for bad in [
            "http://rustybucket.ai",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://",
            "https:///x",
            "https://a b.com",
            "https://a.com/\n--help",
            "-https://a.com",
            "",
        ] {
            assert!(!is_web_url(bad), "{bad:?}");
        }
    }
}
