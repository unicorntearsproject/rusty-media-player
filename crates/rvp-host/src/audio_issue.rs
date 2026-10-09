//! Why the sound stopped, in words a person can act on. A host that can tell (the desktop knows the device and the operating system's
//! error; a page knows the state of its audio context) reports an [`AudioIssue`] through [`crate::AudioSink::issue`]; the application
//! shows [`AudioIssue::message`] and the host keeps trying to get the sound back by itself.
use alloc::format;
use alloc::string::{String, ToString};

/// What kind of failure it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioIssueKind {
    /// Another program holds the device exclusively (`EBUSY`, `AUDCLNT_E_DEVICE_IN_USE`): it will work again once that program lets go.
    Busy,
    /// The device is gone (unplugged), or the sound server restarted or went away: it will work again when it is back.
    Gone,
    /// Anything else; the reason is in the message.
    Other,
}

/// A failure of the audio output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioIssue {
    /// Which kind.
    pub kind: AudioIssueKind,
    /// The device, where the host knows it (empty otherwise).
    pub device: String,
    /// The system's own words, for the `Other` message and the log.
    pub reason: String,
}

impl AudioIssue {
    /// An issue classified from the system's error text and, where there is one, its numeric code (an errno).
    pub fn from_error(device: &str, reason: &str, code: Option<i32>) -> Self {
        Self { kind: classify(reason, code), device: device.to_string(), reason: reason.to_string() }
    }

    fn name(&self) -> String {
        if self.device.trim().is_empty() {
            "The audio output".to_string()
        } else {
            self.device.trim().to_string()
        }
    }

    /// What to tell the person. Wraps at any width (plain sentences).
    pub fn message(&self) -> String {
        let name = self.name();
        match self.kind {
            AudioIssueKind::Busy => format!(
                "Another app is using {name} exclusively. Close it or choose another output. Rusty Wave will resume when it's free."
            ),
            AudioIssueKind::Gone => {
                format!(
                    "{name} was unplugged or the sound server restarted. Rusty Wave will resume when it's back."
                )
            }
            AudioIssueKind::Other => {
                let why = self.reason.trim();
                if why.is_empty() {
                    format!("The audio output failed ({name}). Rusty Wave will keep trying.")
                } else {
                    format!("The audio output failed ({name}): {why}. Rusty Wave will keep trying.")
                }
            }
        }
    }

    /// The short note when the sound is back.
    pub fn recovered_message(device: &str) -> String {
        if device.trim().is_empty() {
            "Audio is back".to_string()
        } else {
            format!("Audio back on {}", device.trim())
        }
    }
}

/// Which kind of failure an error is, from the system's text and numeric code (errno values of Linux and macOS; Windows HRESULTs by their
/// documented names).
pub fn classify(text: &str, code: Option<i32>) -> AudioIssueKind {
    // errno: EBUSY 16; ENODEV 19, ENXIO 6, EPIPE 32, ESHUTDOWN 108, ECONNRESET 104, ENOTCONN 107, ECONNREFUSED 111 (the sound server's socket).
    match code {
        Some(16) => return AudioIssueKind::Busy,
        Some(19 | 6 | 32 | 108 | 104 | 107 | 111) => return AudioIssueKind::Gone,
        _ => {}
    }
    let t = text.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
    if has(&[
        "busy",
        "ebusy",
        "device_in_use",
        "device in use",
        "already in use",
        "is in use",
        "exclusive",
        "resource temporarily unavailable",
    ]) {
        AudioIssueKind::Busy
    } else if has(&[
        "devicenotavailable",
        "device not available",
        "not available",
        "no such device",
        "enodev",
        "unplug",
        "disconnect",
        "no longer",
        "device_invalidated",
        "device invalidated",
        "removed",
        "broken pipe",
        "connection refused",
        "connection reset",
        "server",
        "interrupted",
    ]) {
        AudioIssueKind::Gone
    } else {
        AudioIssueKind::Other
    }
}

/// How long to wait before retry number `attempt` (0 for the first): quickly at first, easing to five seconds, never spinning.
pub fn retry_delay_us(attempt: u32) -> i64 {
    const STEPS_MS: [i64; 6] = [1_000, 1_500, 2_000, 3_000, 4_000, 5_000];
    STEPS_MS[(attempt as usize).min(STEPS_MS.len() - 1)] * 1_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_gone_and_the_rest_are_told_apart() {
        for (text, code, want) in [
            ("Device or resource busy", None, AudioIssueKind::Busy),
            ("start node error -16: Device or resource busy", None, AudioIssueKind::Busy),
            ("anything", Some(16), AudioIssueKind::Busy),
            ("AUDCLNT_E_DEVICE_IN_USE", None, AudioIssueKind::Busy),
            ("The requested device is in use (exclusive mode)", None, AudioIssueKind::Busy),
            ("DeviceNotAvailable", None, AudioIssueKind::Gone),
            ("The device is no longer available", None, AudioIssueKind::Gone),
            ("No such device", None, AudioIssueKind::Gone),
            ("anything", Some(19), AudioIssueKind::Gone),
            ("Connection refused (pulse)", None, AudioIssueKind::Gone),
            ("AUDCLNT_E_DEVICE_INVALIDATED", None, AudioIssueKind::Gone),
            ("the sound server went away", None, AudioIssueKind::Gone),
            ("unsupported sample format F64", None, AudioIssueKind::Other),
            ("", None, AudioIssueKind::Other),
        ] {
            assert_eq!(classify(text, code), want, "{text:?} {code:?}");
        }
        // The numeric code wins over the words.
        assert_eq!(classify("device is gone", Some(16)), AudioIssueKind::Busy);
    }

    #[test]
    fn the_messages_name_the_device_and_the_next_step() {
        let busy = AudioIssue::from_error("DDJ-REV1", "Device or resource busy", None);
        assert_eq!(
            busy.message(),
            "Another app is using DDJ-REV1 exclusively. Close it or choose another output. Rusty Wave will resume when it's free."
        );
        let gone = AudioIssue::from_error("DDJ-REV1", "DeviceNotAvailable", None);
        assert_eq!(
            gone.message(),
            "DDJ-REV1 was unplugged or the sound server restarted. Rusty Wave will resume when it's back."
        );
        let other = AudioIssue::from_error("Speakers", "unsupported sample format F64", None);
        assert!(
            other.message().contains("Speakers") && other.message().contains("unsupported sample format F64")
        );
        // A host that does not know the device still makes sense.
        assert!(
            AudioIssue::from_error("", "busy", None)
                .message()
                .starts_with("Another app is using The audio output")
        );
        assert_eq!(AudioIssue::recovered_message("DDJ-REV1"), "Audio back on DDJ-REV1");
        assert_eq!(AudioIssue::recovered_message(""), "Audio is back");
    }

    #[test]
    fn retries_start_quick_ease_to_five_seconds_and_stay_there() {
        let d: alloc::vec::Vec<i64> = (0..9).map(retry_delay_us).collect();
        assert_eq!(d[0], 1_000_000);
        assert!(d.windows(2).all(|w| w[1] >= w[0]));
        assert_eq!(*d.last().unwrap(), 5_000_000);
        assert_eq!(retry_delay_us(10_000), 5_000_000);
    }
}
