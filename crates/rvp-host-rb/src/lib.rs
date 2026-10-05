//! Rusty Bucket host adapter. Blocked until `../rust-os` has an app ABI with Canvas, input, timers,
//! fs and audio (Milestone 10).

/// Name of the host, shown in diagnostics.
pub const HOST_NAME: &str = "rusty-bucket";

#[cfg(test)]
mod tests {
    #[test]
    fn host_name() {
        assert_eq!(super::HOST_NAME, "rusty-bucket");
    }
}
