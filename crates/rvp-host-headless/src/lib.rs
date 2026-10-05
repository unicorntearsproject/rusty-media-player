//! Native headless host for tests: a virtual-time clock, scripted input, and (from M3/M4) a file
//! `Source`, a WAV/null audio sink and a frame-hashing video sink.
pub use rvp_host::mock::{FakeClock, ScriptedInput};

/// Version string shown by the CLI.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
