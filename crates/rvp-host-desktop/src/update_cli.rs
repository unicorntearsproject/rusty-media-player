//! `--update-now` and `--integration`: the update and the app-menu entry from the command line, without a window (for scripts and tests).
use crate::cli::Options;
use crate::services::{DesktopServices, describe, manifest_url};
use rvp_host::{AppServices, Integration};
use rvp_update::{Cancel, Checked, Offer};
use std::ffi::OsString;

/// Check for an update, install it, and say what happened. Exit status: 0 installed, 10 up to date, 11 cannot apply here, 1 failed.
pub fn update_now(opts: &Options) -> i32 {
    let url = manifest_url(opts.update_manifest.as_deref());
    let cfg = crate::services::config(
        &url,
        std::env::args_os().skip(1).filter(|a| a != "--update-now").collect::<Vec<OsString>>(),
    );
    println!("rusty-wave {} ({}), manifest {url}", env!("CARGO_PKG_VERSION"), describe(&cfg.kind));
    let offer = match rvp_update::updater::check(&cfg) {
        Ok(Checked::UpToDate(v)) => {
            println!("up to date (latest is {v})");
            return 10;
        }
        Ok(Checked::Newer(o)) => o,
        Err(e) => {
            eprintln!("rusty-wave: could not check for updates: {e}");
            return 1;
        }
    };
    match offer {
        Offer::Manual { release, message, url } => {
            println!(
                "{} is available. {message}{}",
                release.version,
                url.map(|u| format!(" {u}")).unwrap_or_default()
            );
            11
        }
        Offer::Update { release, file } => {
            println!("{} is available: downloading {}", release.version, file.name);
            let mut last = 0u64;
            let mut progress = |done: u64, total: u64| {
                if done == total || done >= last + total.max(1) / 10 {
                    last = done;
                    println!("  {done} / {total} bytes");
                }
            };
            match rvp_update::updater::install(&cfg, &file, &Cancel::new(), &mut progress) {
                Ok(f) => {
                    println!("installed and verified ({f:?}); it runs from the next start");
                    0
                }
                Err(e) => {
                    eprintln!("rusty-wave: the update was not installed: {e}");
                    1
                }
            }
        }
    }
}

/// Add, remove or show the app-menu entry. Exit status 0 on success (status: 0 in the menu, 10 not), 1 on failure, 11 where there is none.
pub fn integration(opts: &Options, action: &str) -> i32 {
    let mut svc = DesktopServices::new(&manifest_url(opts.update_manifest.as_deref()), Vec::new());
    println!("{} installation", describe(svc.kind()));
    let state = svc.integration();
    match action {
        "status" => match state {
            Integration::Unavailable => {
                println!("not applicable here");
                11
            }
            Integration::On => {
                println!("in the app menu");
                0
            }
            Integration::Off => {
                println!("not in the app menu");
                10
            }
        },
        _ => {
            if state == Integration::Unavailable {
                eprintln!("rusty-wave: this installation has no app-menu entry to change");
                return 11;
            }
            match svc.set_integration(action == "add") {
                Ok(()) => {
                    println!(
                        "{}",
                        if action == "add" { "added to the app menu" } else { "removed from the app menu" }
                    );
                    0
                }
                Err(e) => {
                    eprintln!("rusty-wave: {e}");
                    1
                }
            }
        }
    }
}
