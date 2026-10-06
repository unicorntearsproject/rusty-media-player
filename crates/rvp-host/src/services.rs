//! The optional app-services capability: update checks and adding the app to the desktop's menus.
//!
//! Only the desktop host has it (a browser updates through its service worker, Rusty Bucket through its own store), so the player offers
//! these entries only when [`crate::Host::app_services`] returns something. The host does the work on its own threads; the player
//! polls [`AppServices::update_state`] every tick and draws what it finds.
use alloc::string::String;
use alloc::vec::Vec;

/// How to get the newer version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateHow {
    /// The app can download, verify and install it itself.
    Install,
    /// The user does it (a package manager, a store, a download): say how.
    Manual {
        /// What to tell the user.
        message: String,
        /// A download address, where one makes sense.
        url: Option<String>,
    },
}

/// What the update service is doing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UpdateState {
    /// Nothing asked yet.
    #[default]
    Idle,
    /// Looking for a newer version.
    Checking,
    /// Nothing newer than what runs.
    UpToDate,
    /// Updates of this installation come from elsewhere (Flatpak, a store); no check is made, here is what to do.
    Managed(String),
    /// A newer version exists.
    Available {
        /// Its version.
        version: String,
        /// How to get it.
        how: UpdateHow,
    },
    /// Downloading it.
    Downloading {
        /// Bytes so far.
        done: u64,
        /// Bytes in all.
        total: u64,
    },
    /// Checking the download and putting it in place.
    Installing,
    /// Installed; restarting finishes the job.
    Ready {
        /// The installed version.
        version: String,
    },
    /// A step failed (the text is for the user).
    Failed(String),
}

/// Whether the app is in the desktop's app menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Integration {
    /// This kind of install has no use for it (a package or Flatpak does it already, a browser cannot).
    #[default]
    Unavailable,
    /// Not in the app menu.
    Off,
    /// In the app menu.
    On,
}

/// Update and app-menu services.
pub trait AppServices {
    /// The running version, `0.0.3`.
    fn version(&self) -> String;
    /// Seconds since the Unix epoch (the player's own clock is monotonic).
    fn unix_time(&self) -> i64;
    /// Whether "check for updates" is offered at all.
    fn updates_supported(&self) -> bool;
    /// The state of the update service (cheap; called every tick).
    fn update_state(&mut self) -> UpdateState;
    /// Start looking for a newer version (ignored while busy).
    fn check_updates(&mut self);
    /// Download and install the version on offer.
    fn install_update(&mut self);
    /// Stop a download.
    fn cancel_update(&mut self);
    /// Forget a failure or an up-to-date answer, back to what was on offer (or nothing).
    fn reset_update(&mut self);
    /// Start the new version; the host quits afterwards when this returns `Ok`.
    fn restart(&mut self) -> Result<(), String>;
    /// Whether the app is in the desktop's app menu.
    fn integration(&mut self) -> Integration;
    /// Add it (`true`) or take it out; errors are for the user.
    fn set_integration(&mut self, on: bool) -> Result<(), String>;
}

/// A scripted [`AppServices`] for tests: the test sets the state, the player's requests are recorded.
#[derive(Debug, Default)]
pub struct ScriptedServices {
    /// What `version` returns.
    pub version: String,
    /// What `unix_time` returns.
    pub now: i64,
    /// What `updates_supported` returns.
    pub updates: bool,
    /// What `update_state` returns.
    pub state: UpdateState,
    /// What `integration` returns (and `set_integration` changes).
    pub integration: Integration,
    /// An error `set_integration` returns instead of doing it.
    pub integration_error: Option<String>,
    /// An error `restart` returns.
    pub restart_error: Option<String>,
    /// What `check_updates` turns the state into (set to simulate the answer arriving).
    pub check_result: Option<UpdateState>,
    /// The requests received, in order: `check`, `install`, `cancel`, `reset`, `restart`, `integrate:on|off`.
    pub calls: Vec<String>,
}

impl AppServices for ScriptedServices {
    fn version(&self) -> String {
        self.version.clone()
    }
    fn unix_time(&self) -> i64 {
        self.now
    }
    fn updates_supported(&self) -> bool {
        self.updates
    }
    fn update_state(&mut self) -> UpdateState {
        self.state.clone()
    }
    fn check_updates(&mut self) {
        self.calls.push("check".into());
        self.state = self.check_result.clone().unwrap_or(UpdateState::Checking);
    }
    fn install_update(&mut self) {
        self.calls.push("install".into());
        self.state = UpdateState::Downloading { done: 0, total: 100 };
    }
    fn cancel_update(&mut self) {
        self.calls.push("cancel".into());
    }
    fn reset_update(&mut self) {
        self.calls.push("reset".into());
        self.state = UpdateState::Idle;
    }
    fn restart(&mut self) -> Result<(), String> {
        self.calls.push("restart".into());
        match &self.restart_error {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }
    fn integration(&mut self) -> Integration {
        self.integration
    }
    fn set_integration(&mut self, on: bool) -> Result<(), String> {
        self.calls.push(if on { "integrate:on" } else { "integrate:off" }.into());
        if let Some(e) = &self.integration_error {
            return Err(e.clone());
        }
        self.integration = if on { Integration::On } else { Integration::Off };
        Ok(())
    }
}
