//! The player engine: command/event API, state machine, and (from M1) the cooperative scheduler,
//! pipeline and A/V sync. See `docs/PLAN.md` sections 5-7.
#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod audio;
pub mod exec;
pub mod session;

pub use audio::{AudioOut, TraceEntry};
pub use session::{
    EXTERNAL_TRACK_BASE, Session, SessionEvent, SessionState, SubtitleTrack, VideoStats, VideoTraceEntry,
    language_name,
};

use alloc::collections::VecDeque;
use rvp_core::Timestamp;

/// Playback state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Nothing loaded.
    Idle,
    /// Loaded and paused.
    Paused,
    /// Playing.
    Playing,
    /// Reached the end of the item.
    Ended,
}

/// Commands from the UI or the host.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Start or resume.
    Play,
    /// Pause.
    Pause,
    /// Toggle play/pause.
    TogglePause,
    /// Seek to an absolute position.
    Seek(Timestamp),
    /// Playback rate, 1.0 = normal.
    SetRate(f64),
    /// Volume, 0.0..=1.0.
    SetVolume(f32),
    /// Stop and unload.
    Stop,
}

/// Events for the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// State changed.
    StateChanged(State),
    /// Rate changed.
    RateChanged(f64),
    /// Volume changed.
    VolumeChanged(f32),
}

/// A read-only view for the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// State.
    pub state: State,
    /// Rate.
    pub rate: f64,
    /// Volume.
    pub volume: f32,
}

/// The player. In M0 it is only the state machine.
#[derive(Debug)]
pub struct Player {
    state: State,
    rate: f64,
    volume: f32,
    events: VecDeque<Event>,
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

impl Player {
    /// A new idle player.
    pub fn new() -> Self {
        Self { state: State::Idle, rate: 1.0, volume: 1.0, events: VecDeque::new() }
    }

    /// Read-only state for the UI.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot { state: self.state, rate: self.rate, volume: self.volume }
    }

    /// Next pending event.
    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// Apply a command. Commands that make no sense in the current state are ignored.
    pub fn command(&mut self, cmd: Command) {
        match cmd {
            Command::Play => self.set_state(State::Playing, &[State::Paused, State::Ended]),
            Command::Pause => self.set_state(State::Paused, &[State::Playing]),
            Command::TogglePause => match self.state {
                State::Playing => self.set_state(State::Paused, &[State::Playing]),
                State::Paused | State::Ended => {
                    self.set_state(State::Playing, &[State::Paused, State::Ended])
                }
                State::Idle => {}
            },
            Command::SetRate(r) => {
                self.rate = r.clamp(0.25, 4.0);
                self.events.push_back(Event::RateChanged(self.rate));
            }
            Command::SetVolume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                self.events.push_back(Event::VolumeChanged(self.volume));
            }
            Command::Stop => self.set_state(State::Idle, &[State::Paused, State::Playing, State::Ended]),
            Command::Seek(_) => {}
        }
    }

    fn set_state(&mut self, to: State, from: &[State]) {
        if from.contains(&self.state) && self.state != to {
            self.state = to;
            self.events.push_back(Event::StateChanged(to));
        }
    }

    /// Test/bring-up helper until `Open` exists (M1): pretend an item is loaded and paused.
    #[doc(hidden)]
    pub fn debug_load(&mut self) {
        self.set_state(State::Paused, &[State::Idle]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_pause_cycle_emits_events() {
        let mut p = Player::new();
        p.command(Command::Play); // nothing loaded: ignored
        assert_eq!(p.snapshot().state, State::Idle);
        p.debug_load();
        p.command(Command::TogglePause);
        p.command(Command::TogglePause);
        let evs: alloc::vec::Vec<_> = core::iter::from_fn(|| p.poll_event()).collect();
        assert_eq!(
            evs,
            [
                Event::StateChanged(State::Paused),
                Event::StateChanged(State::Playing),
                Event::StateChanged(State::Paused)
            ]
        );
    }

    #[test]
    fn rate_and_volume_are_clamped() {
        let mut p = Player::new();
        p.command(Command::SetRate(100.0));
        p.command(Command::SetVolume(-1.0));
        assert_eq!((p.snapshot().rate, p.snapshot().volume), (4.0, 0.0));
    }
}
