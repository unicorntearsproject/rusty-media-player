//! Raw bindings to Rusty Bucket's App API, module `bucket_v0` (draft v0.3).
//!
//! Self-contained: no dependencies, no knowledge of any application. It holds
//!
//! - every import ([`FUNCTIONS`] lists them with their WebAssembly signatures; on wasm32 they are `extern "C"` functions of
//!   module `bucket_v0`),
//! - the `#[repr(C)]` structs that cross the boundary, with compile-time size and offset asserts that match the offset
//!   tables of the API pages ([`structs`]),
//! - the constants: error codes, capability bits, event kinds, keys, limits ([`consts`] items are re-exported at the top),
//! - on native targets with the feature `backend`, a way to run the same code against a [`Backend`] (mock host, simulator),
//! - with the feature `inspect`, a parser for the import section of a module and the check of its imports against
//!   [`FUNCTIONS`] ([`inspect`]).
//!
//! Pointers and lengths are `u32` offsets into linear memory on wasm32 (a raw pointer there), so the declarations use raw
//! pointers and `i32` lengths exactly as the pages do. Every function always exists in a major version; a host that predates
//! one answers `-UNSUPPORTED`.
#![no_std]

#[cfg(feature = "inspect")]
extern crate alloc;
#[cfg(any(feature = "backend", test))]
extern crate std;

mod consts;
mod funcs;
mod structs;

#[cfg(all(not(target_arch = "wasm32"), feature = "backend"))]
pub mod backend;
#[cfg(feature = "inspect")]
pub mod inspect;

pub use consts::*;
pub use funcs::*;
pub use structs::*;

/// Name of the import module.
pub const MODULE: &str = "bucket_v0";

/// The minor revision this crate was written against (what `api_version()` returns on a host at this revision).
pub const API_MINOR: i32 = 3;

/// A pointer as the structs carry it: a `u32` offset into linear memory. On wasm32 this is the address itself.
#[cfg(not(all(not(target_arch = "wasm32"), feature = "backend")))]
pub fn ptr32<T>(p: *const T) -> u32 {
    p as usize as u32
}

/// A pointer as the structs carry it. On native targets with the `backend` feature this is a token for the current call; use
/// [`backend::deref32`] to get the address back.
#[cfg(all(not(target_arch = "wasm32"), feature = "backend"))]
pub fn ptr32<T>(p: *const T) -> u32 {
    backend::make_token(p as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_table_is_consistent() {
        assert!(FUNCTIONS.len() > 50);
        let mut names: std::vec::Vec<_> = FUNCTIONS.iter().map(|f| f.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), FUNCTIONS.len(), "duplicate function name");
        let sig = |n: &str| FUNCTIONS.iter().find(|f| f.name == n).unwrap();
        assert_eq!(sig("events_wait").params, &["i32", "i32", "i64"]);
        assert_eq!(sig("events_wait").result, "i32");
        assert_eq!(sig("time_now_us").result, "i64");
        assert_eq!(sig("exit").result, "");
        assert_eq!(sig("audio_volume").params, &["i32", "f32"]);
        assert_eq!(sig("library_listing").params, &["i32", "i32", "i64", "i32", "i32"]);
        assert_eq!(sig("library_listing").result, "i64");
    }

    #[test]
    fn event_fields_round_trip() {
        let mut e = Event::new(ev::TRANSPORT, 0, 123);
        e.put_u32(16, transport::SEEK_TO);
        e.put_i64(24, -5_000_000);
        let back = Event::from_bytes(&e.to_bytes());
        assert_eq!(back, e);
        assert_eq!((back.kind, back.time_us), (32, 123));
        assert_eq!(back.u32_at(16), 6);
        assert_eq!(back.i64_at(24), -5_000_000);
        // A read past the payload, or before it, is zero, never a panic.
        assert_eq!(back.u32_at(62), 0);
        assert_eq!(back.u32_at(8), 0);
    }

    #[test]
    fn viz_default_has_its_size() {
        assert_eq!(VizSummaryRaw::default().struct_size, 176);
    }
}
