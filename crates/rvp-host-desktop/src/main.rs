//! `rusty-wave`: Rusty Wave.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    attach_console();
    let code = rvp_host_desktop::main_with_args(std::env::args().skip(1).collect());
    std::process::exit(code);
}

/// A Windows GUI program has no console; when started from one, print to it (`rusty-wave --help`, errors).
#[cfg(windows)]
fn attach_console() {
    // SAFETY: a plain Win32 call with no pointers; failing (no parent console) is fine.
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(
            windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
        );
    }
}

#[cfg(not(windows))]
fn attach_console() {}
