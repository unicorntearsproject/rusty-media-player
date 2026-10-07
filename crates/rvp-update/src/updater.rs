//! The update service: check the manifest, offer what is newer, install it on a background thread, report progress.
use crate::apply::{self, Finish, Job};
use crate::error::UpdateError;
use crate::fetch::{Cancel, Source, fetch_bytes};
use crate::kind::InstallKind;
use crate::manifest::{FileEntry, Manifest};
use crate::verify::Verifier;
use crate::version::Version;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Where the latest release is described.
pub const MANIFEST_URL: &str = "https://software.rustybucket.ai/rusty-wave/latest/rusty-wave-latest.json";
const MAX_MANIFEST: u64 = 256 * 1024;
const MAX_SIGNATURE: u64 = 64 * 1024;

/// What the service needs to know about this copy of the program.
#[derive(Clone)]
pub struct Config {
    /// The manifest (`https://`, or a `file://` URL or a path for tests).
    pub manifest_url: String,
    /// The running version.
    pub current: Version,
    /// How this copy is installed.
    pub kind: InstallKind,
    /// The key to trust.
    pub verifier: Arc<Verifier>,
    /// Where bytes come from.
    pub source: Arc<dyn Source>,
    /// Where Setup programs and zips go before they are checked.
    pub temp_dir: PathBuf,
    /// The arguments to start the new version with.
    pub restart_args: Vec<OsString>,
}

/// A release newer than the running one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Its version.
    pub version: Version,
    /// Release date, if the manifest says.
    pub released: String,
}

/// What to do about a newer release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    /// This copy can download and install it.
    Update {
        /// The release.
        release: Release,
        /// The file that carries it for this install.
        file: FileEntry,
    },
    /// This copy cannot: tell the user how.
    Manual {
        /// The release.
        release: Release,
        /// What to say.
        message: String,
        /// A file to download, where one makes sense.
        url: Option<String>,
    },
}

impl Offer {
    /// The release on offer.
    pub fn release(&self) -> &Release {
        match self {
            Offer::Update { release, .. } | Offer::Manual { release, .. } => release,
        }
    }
}

/// The result of looking at the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checked {
    /// Nothing newer (the version the manifest names).
    UpToDate(Version),
    /// Something newer.
    Newer(Offer),
}

/// Fetch the manifest and its signature, trust it only if the release key signed it, and compare versions.
pub fn check(cfg: &Config) -> Result<Checked, UpdateError> {
    let body = fetch_bytes(&*cfg.source, &cfg.manifest_url, MAX_MANIFEST)?;
    let sig = fetch_bytes(&*cfg.source, &format!("{}.asc", cfg.manifest_url), MAX_SIGNATURE)?;
    cfg.verifier.verify_detached(&body, &sig)?;
    let m = Manifest::parse(&body)?;
    let want: String = m.key_fingerprint.chars().filter(|c| !c.is_whitespace()).collect();
    if !want.eq_ignore_ascii_case(&cfg.verifier.fingerprint()) {
        return Err(UpdateError::UntrustedKey);
    }
    let latest = Version::parse(&m.version)
        .ok_or_else(|| UpdateError::BadManifest(format!("`{}` is not a version", m.version)))?;
    // A validly signed older manifest (a replayed one) never offers a downgrade.
    if latest <= cfg.current {
        return Ok(Checked::UpToDate(latest));
    }
    let release = Release { version: latest, released: m.released.clone() };
    let entry = cfg.kind.manifest_key().and_then(|k| m.files.get(k));
    if let Some(e) = entry {
        crate::apply::fetch_name_ok(&e.name)?;
    }
    if cfg.kind.can_self_update()
        && let Some(file) = entry
    {
        return Ok(Checked::Newer(Offer::Update { release, file: file.clone() }));
    }
    let url = match cfg.kind {
        InstallKind::MacApp => entry.map(|e| e.url.clone()),
        _ => None,
    };
    Ok(Checked::Newer(Offer::Manual { release, message: cfg.kind.manual_message().to_string(), url }))
}

/// Install an [`Offer::Update`]: download, verify, put in place. Returns what is left to do.
pub fn install(
    cfg: &Config,
    file: &FileEntry,
    cancel: &Cancel,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Finish, UpdateError> {
    let job = Job {
        source: &*cfg.source,
        verifier: &cfg.verifier,
        kind: &cfg.kind,
        temp_dir: &cfg.temp_dir,
        entry: file,
        cancel,
    };
    apply::install(&job, progress)
}

/// What the service is doing, for the UI to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Nothing asked yet.
    Idle,
    /// Looking at the manifest.
    Checking,
    /// Nothing newer than the running version.
    UpToDate(Version),
    /// A newer release.
    Available(Offer),
    /// Downloading the update.
    Downloading {
        /// Bytes so far.
        done: u64,
        /// Bytes in all.
        total: u64,
    },
    /// Checking the download and putting it in place.
    Installing,
    /// Installed; a restart finishes it.
    Ready(Release),
    /// A step failed (the text is for the user).
    Failed(String),
}

struct Inner {
    state: State,
    offer: Option<Offer>,
    finish: Option<Finish>,
    cancel: Cancel,
    busy: bool,
}

/// The update service: calls return at once, work happens on a thread, [`Updater::state`] reports.
#[derive(Clone)]
pub struct Updater {
    cfg: Arc<Config>,
    inner: Arc<Mutex<Inner>>,
}

impl Updater {
    /// A service for this copy of the program.
    pub fn new(cfg: Config) -> Updater {
        apply::clean_leftovers(&cfg.kind);
        Updater {
            cfg: Arc::new(cfg),
            inner: Arc::new(Mutex::new(Inner {
                state: State::Idle,
                offer: None,
                finish: None,
                cancel: Cancel::new(),
                busy: false,
            })),
        }
    }

    /// The configuration.
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// The current state.
    pub fn state(&self) -> State {
        self.inner.lock().map(|i| i.state.clone()).unwrap_or(State::Idle)
    }

    /// Start looking at the manifest (ignored while something else is running).
    pub fn check(&self) {
        let Ok(mut g) = self.inner.lock() else { return };
        if g.busy {
            return;
        }
        g.busy = true;
        g.state = State::Checking;
        drop(g);
        let me = self.clone();
        std::thread::spawn(move || {
            let r = check(&me.cfg);
            let Ok(mut g) = me.inner.lock() else { return };
            g.busy = false;
            match r {
                Ok(Checked::UpToDate(v)) => {
                    g.offer = None;
                    g.state = State::UpToDate(v);
                }
                Ok(Checked::Newer(o)) => {
                    g.offer = Some(o.clone());
                    g.state = State::Available(o);
                }
                Err(e) => {
                    g.state = State::Failed(format!("Couldn't check for updates: {e}. {}", check_advice(&e)))
                }
            }
        });
    }

    /// Start downloading and installing the offered update (needs an [`Offer::Update`]).
    pub fn install(&self) {
        let Ok(mut g) = self.inner.lock() else { return };
        let Some(Offer::Update { release, file }) = g.offer.clone() else { return };
        if g.busy {
            return;
        }
        g.busy = true;
        g.cancel = Cancel::new();
        let cancel = g.cancel.clone();
        g.state = State::Downloading { done: 0, total: file.size };
        drop(g);
        let me = self.clone();
        std::thread::spawn(move || {
            let me2 = me.clone();
            let mut last = 0u64;
            let mut progress = move |done: u64, total: u64| {
                // Every 64 KiB at most is plenty for a progress bar.
                if done == total || done.saturating_sub(last) >= 64 * 1024 || done == 0 {
                    last = done;
                    if let Ok(mut g) = me2.inner.lock()
                        && matches!(g.state, State::Downloading { .. })
                    {
                        // Everything is here: what follows is checking it and putting it in place.
                        g.state = if total > 0 && done >= total {
                            State::Installing
                        } else {
                            State::Downloading { done, total }
                        };
                    }
                }
            };
            let r = install(&me.cfg, &file, &cancel, &mut progress);
            let Ok(mut g) = me.inner.lock() else { return };
            g.busy = false;
            match r {
                Ok(f) => {
                    g.finish = Some(f);
                    g.state = State::Ready(release);
                }
                Err(UpdateError::Cancelled) => {
                    g.state = State::Available(Offer::Update { release, file });
                }
                Err(e) => {
                    g.state = State::Failed(format!(
                        "The update wasn't installed: {e}. Nothing was changed; you can try again, or download the new version from software.rustybucket.ai/rusty-wave/latest/."
                    ))
                }
            }
        });
    }

    /// Stop a download (the offer stays).
    pub fn cancel(&self) {
        if let Ok(g) = self.inner.lock() {
            g.cancel.cancel();
        }
    }

    /// Start the new version (or its installer); the caller quits when this returns `Ok`.
    pub fn restart(&self) -> Result<(), UpdateError> {
        let finish = self.inner.lock().ok().and_then(|g| g.finish.clone());
        let Some(f) = finish else {
            return Err(UpdateError::Io("there is no installed update to start".into()));
        };
        apply::run_finish(&f, &self.cfg.restart_args)
    }

    /// Give the state a reason to be shown again (after the user dismissed a failure).
    pub fn reset(&self) {
        if let Ok(mut g) = self.inner.lock()
            && !g.busy
        {
            g.state = match &g.offer {
                Some(o) => State::Available(o.clone()),
                None => State::Idle,
            };
        }
    }
}

/// What to do next after a failed check, by what failed.
fn check_advice(e: &UpdateError) -> &'static str {
    match e {
        UpdateError::Io(_) | UpdateError::Http(_) => "Check your internet connection and try again.",
        _ => {
            "Try again later; if it keeps happening, download Rusty Wave from software.rustybucket.ai/rusty-wave/latest/."
        }
    }
}
