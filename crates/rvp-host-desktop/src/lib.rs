//! The desktop host (Linux and Windows) and the `rusty-wave` application: a `winit` window, `softbuffer` pixels, `cpal` audio, `rfd`
//! dialogs, drag and drop, full screen, HiDPI, the system media controls (MPRIS, SMTC), real files for the library and settings, and
//! decoding on worker threads.
//!
//! It implements the host traits of `rvp-host` and runs `rvp_app::App`; nothing in it knows about browsers or Rusty Bucket.
#![deny(missing_docs)]

pub mod audio;
pub mod cli;
pub mod codecs;
pub mod dialogs;
pub mod fonts;
pub mod host;
pub mod input;
pub mod links;
pub mod media;
pub mod net;
pub mod platform;
pub mod services;
pub mod smoke;
pub mod source;
pub mod storage;
pub mod update_cli;
pub mod walk;
pub mod window;
pub mod writer;

pub use links::open_url;

pub use window::{APP_ID, APP_NAME};

/// The program: parse the command line, run the window, return the exit code.
pub fn main_with_args(args: Vec<String>) -> i32 {
    let opts = match cli::parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("rusty-wave: {e}");
            return 2;
        }
    };
    if opts.help {
        print!("{}", cli::HELP);
        return 0;
    }
    if opts.version {
        println!("rusty-wave {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    if opts.update_now {
        return update_cli::update_now(&opts);
    }
    if let Some(action) = opts.integration.clone() {
        return update_cli::integration(&opts, &action);
    }
    let dir = window::data_dir(&opts);
    window::run(opts, dir)
}
