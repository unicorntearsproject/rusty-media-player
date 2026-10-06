//! The system's media controls: MPRIS on Linux (`playerctl`, the desktop's media widget and the media keys) and the System Media
//! Transport Controls on Windows (the volume flyout, the lock screen and the media keys), through `souvlaki`.
use rvp_host::{NowPlaying, NowPlayingMeta, PlayState, Playback, TransportCommand};
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

/// The host side of [`NowPlaying`].
pub struct DesktopNowPlaying {
    controls: MediaControls,
    rx: Receiver<TransportCommand>,
    raise: Receiver<Raise>,
    cache_dir: PathBuf,
    cover: Option<PathBuf>,
    /// The latest playback state and when it was sent: MPRIS clients (`playerctl`, a shell's progress bar) read the position
    /// as a plain number and do not extrapolate it, so while playing it is sent again every second with the time that passed.
    last_playback: Option<(Playback, Instant)>,
}

/// Something the system asked of the window rather than of playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Raise {
    /// Bring the window to the front.
    Window,
    /// Close the application.
    Quit,
}

/// Map a media-control event to a transport command (the pure part, tested without a bus).
pub fn map_event(e: &MediaControlEvent, position_us: i64) -> Option<TransportCommand> {
    use TransportCommand as T;
    Some(match e {
        MediaControlEvent::Play => T::Play,
        MediaControlEvent::Pause => T::Pause,
        MediaControlEvent::Toggle => T::Toggle,
        MediaControlEvent::Next => T::Next,
        MediaControlEvent::Previous => T::Prev,
        MediaControlEvent::Stop => T::Stop,
        MediaControlEvent::Seek(SeekDirection::Forward) => T::SeekBy(10_000_000),
        MediaControlEvent::Seek(SeekDirection::Backward) => T::SeekBy(-10_000_000),
        MediaControlEvent::SeekBy(SeekDirection::Forward, d) => T::SeekBy(d.as_micros() as i64),
        MediaControlEvent::SeekBy(SeekDirection::Backward, d) => T::SeekBy(-(d.as_micros() as i64)),
        MediaControlEvent::SetPosition(MediaPosition(d)) => T::SeekTo(d.as_micros() as i64),
        MediaControlEvent::SetVolume(v) => T::SetVolume(v.clamp(0.0, 1.0) as f32),
        MediaControlEvent::OpenUri(_) | MediaControlEvent::Raise | MediaControlEvent::Quit => {
            let _ = position_us;
            return None;
        }
    })
}

/// True if there is a session bus to talk to. Without one the MPRIS service thread could not start (and would take the process down
/// with it when panics abort), so media controls are simply left out.
pub fn bus_available() -> bool {
    #[cfg(unix)]
    {
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some_and(|v| !v.is_empty()) {
            return true;
        }
        std::env::var_os("XDG_RUNTIME_DIR").is_some_and(|d| std::path::Path::new(&d).join("bus").exists())
    }
    #[cfg(not(unix))]
    {
        true
    }
}

impl DesktopNowPlaying {
    /// Start the controls. `dbus_name` is the part after `org.mpris.MediaPlayer2.` (Linux); `hwnd` the window handle (Windows).
    pub fn new(
        display_name: &str,
        dbus_name: &str,
        hwnd: Option<*mut std::ffi::c_void>,
        cache_dir: PathBuf,
    ) -> Result<Self, String> {
        #[cfg(unix)]
        if !bus_available() {
            return Err("no D-Bus session bus".into());
        }
        let mut controls = MediaControls::new(PlatformConfig { display_name, dbus_name, hwnd })
            .map_err(|e| format!("{e:?}"))?;
        let (tx, rx): (Sender<TransportCommand>, _) = channel();
        let (rtx, raise) = channel();
        controls
            .attach(move |event| {
                match &event {
                    MediaControlEvent::Raise => {
                        let _ = rtx.send(Raise::Window);
                    }
                    MediaControlEvent::Quit => {
                        let _ = rtx.send(Raise::Quit);
                    }
                    _ => {}
                }
                if let Some(c) = map_event(&event, 0) {
                    let _ = tx.send(c);
                }
            })
            .map_err(|e| format!("{e:?}"))?;
        Ok(Self { controls, rx, raise, cache_dir, cover: None, last_playback: None })
    }

    /// A request for the window (raise, quit) from the system, if any.
    pub fn poll_raise(&mut self) -> Option<Raise> {
        self.raise.try_recv().ok()
    }

    fn write_cover(&mut self, art: &rvp_host::Art) -> Option<String> {
        let ext = if art.mime.contains("png") { "png" } else { "jpg" };
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for &b in art.data.iter().take(4096) {
            h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
        }
        let path = self.cache_dir.join(format!("cover-{h:016x}.{ext}"));
        std::fs::create_dir_all(&self.cache_dir).ok()?;
        std::fs::write(&path, &art.data).ok()?;
        if let Some(old) = self.cover.replace(path.clone())
            && old != path
        {
            let _ = std::fs::remove_file(old);
        }
        let p = path.to_string_lossy().replace('\\', "/");
        Some(if p.starts_with('/') { format!("file://{p}") } else { format!("file:///{p}") })
    }
}

impl Drop for DesktopNowPlaying {
    fn drop(&mut self) {
        if let Some(c) = self.cover.take() {
            let _ = std::fs::remove_file(c);
        }
    }
}

impl DesktopNowPlaying {
    fn send(&mut self, p: &Playback, position_us: i64) {
        let progress = Some(MediaPosition(Duration::from_micros(position_us.max(0) as u64)));
        let _ = self.controls.set_playback(match p.state {
            PlayState::Stopped => MediaPlayback::Stopped,
            PlayState::Paused => MediaPlayback::Paused { progress },
            PlayState::Playing => MediaPlayback::Playing { progress },
        });
    }
}

impl NowPlaying for DesktopNowPlaying {
    fn set_metadata(&mut self, meta: &NowPlayingMeta) {
        let cover = meta.art.as_ref().and_then(|a| self.write_cover(a));
        let _ = self.controls.set_metadata(MediaMetadata {
            title: Some(&meta.title),
            artist: (!meta.artist.is_empty()).then_some(meta.artist.as_str()),
            album: (!meta.album.is_empty()).then_some(meta.album.as_str()),
            cover_url: cover.as_deref(),
            duration: meta.duration_us.filter(|d| *d > 0).map(|d| Duration::from_micros(d as u64)),
        });
    }

    fn set_playback(&mut self, p: &Playback) {
        self.last_playback = Some((*p, Instant::now()));
        self.send(p, p.position_us);
    }

    fn set_volume(&mut self, volume: f32) {
        // MPRIS has a Volume property (and announces the change); the Windows and macOS controls have none.
        #[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
        let _ = self.controls.set_volume(f64::from(volume));
        #[cfg(not(all(unix, not(any(target_os = "macos", target_os = "ios")))))]
        let _ = volume;
    }

    fn poll_command(&mut self) -> Option<TransportCommand> {
        // Called every tick: the moment to refresh a position that would otherwise be stale.
        if let Some((p, sent)) = self.last_playback
            && p.state == PlayState::Playing
            && sent.elapsed() >= Duration::from_secs(1)
        {
            let pos = p.position_us + (sent.elapsed().as_micros() as f64 * p.rate as f64) as i64;
            self.send(&p, pos);
            self.last_playback = Some((Playback { position_us: pos, ..p }, Instant::now()));
        }
        self.rx.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_map_to_commands() {
        assert_eq!(map_event(&MediaControlEvent::Toggle, 0), Some(TransportCommand::Toggle));
        assert_eq!(map_event(&MediaControlEvent::Previous, 0), Some(TransportCommand::Prev));
        assert_eq!(
            map_event(&MediaControlEvent::SeekBy(SeekDirection::Backward, Duration::from_secs(5)), 0),
            Some(TransportCommand::SeekBy(-5_000_000))
        );
        assert_eq!(
            map_event(&MediaControlEvent::SetPosition(MediaPosition(Duration::from_millis(1500))), 0),
            Some(TransportCommand::SeekTo(1_500_000))
        );
        assert_eq!(map_event(&MediaControlEvent::SetVolume(3.0), 0), Some(TransportCommand::SetVolume(1.0)));
        assert_eq!(map_event(&MediaControlEvent::Raise, 0), None);
    }
}
