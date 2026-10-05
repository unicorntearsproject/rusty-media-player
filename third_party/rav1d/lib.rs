#![allow(warnings)] // vendored code: upstream lints are not ours to fix (see PATCHES.md)
#![allow(non_upper_case_globals)]
#![cfg_attr(target_arch = "arm", feature(stdarch_arm_feature_detection))]
#![cfg_attr(
    any(target_arch = "riscv32", target_arch = "riscv64"),
    feature(stdarch_riscv_feature_detection)
)]
#![deny(unsafe_op_in_unsafe_fn)]
#![allow(clippy::all)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![deny(clippy::missing_safety_doc)]

#[cfg(not(any(feature = "bitdepth_8", feature = "bitdepth_16")))]
compile_error!("No bitdepths enabled. Enable one or more of the following features: `bitdepth_8`, `bitdepth_16`");

/// Minimal replacement for the `libc` crate items rav1d uses, so the crate builds for targets with no libc
/// (wasm32-unknown-unknown). The errno values are the Linux ones, which is what rav1d's C API documents.
/// This is the only substantive change from upstream rav1d 1.1.0 besides the notes in PATCHES.md.
#[allow(non_camel_case_types)]
pub(crate) mod libc {
    pub type ptrdiff_t = isize;
    pub type intptr_t = isize;
    pub type uintptr_t = usize;
    pub type off_t = i64;
    pub const ENOENT: i32 = 2;
    pub const EIO: i32 = 5;
    pub const EAGAIN: i32 = 11;
    pub const ENOMEM: i32 = 12;
    pub const EINVAL: i32 = 22;
    pub const ERANGE: i32 = 34;
    pub const ENOPROTOOPT: i32 = 92;
}

pub mod include {
    pub mod common {
        pub(crate) mod attributes;
        pub(crate) mod bitdepth;
        pub(crate) mod dump;
        pub(crate) mod intops;
        pub(crate) mod validate;
    } // mod common
    pub mod dav1d {
        pub mod common;
        pub mod data;
        pub mod dav1d;
        pub mod headers;
        pub mod picture;
    } // mod dav1d
} // mod include
pub mod src {
    pub mod align;
    pub(crate) mod assume;
    pub(crate) mod c_arc;
    pub(crate) mod c_box;
    mod cdef;
    mod cdef_apply;
    mod cdf;
    mod const_fn;
    pub mod cpu;
    mod ctx;
    mod cursor;
    mod data;
    mod decode;
    mod dequant_tables;
    pub(crate) mod disjoint_mut;
    pub(crate) mod enum_map;
    mod env;
    pub(crate) mod error;
    mod ffi_safe;
    mod fg_apply;
    mod filmgrain;
    mod getbits;
    pub(crate) mod pic_or_buf;
    pub(crate) mod pixels;
    pub(crate) mod relaxed_atomic;
    pub mod send_sync_non_null;
    pub(crate) mod strided;
    pub(crate) mod with_offset;
    pub(crate) mod wrap_fn_ptr;
    // TODO(kkysen) Temporarily `pub(crate)` due to a `pub use` until TAIT.
    mod extensions;
    mod in_range;
    pub(super) mod internal;
    mod intra_edge;
    mod ipred;
    mod ipred_prepare;
    mod iter;
    mod itx;
    mod itx_1d;
    pub(crate) mod levels;
    mod lf_apply;
    mod lf_mask;
    pub mod lib;
    pub(crate) mod log;
    mod loopfilter;
    mod looprestoration;
    mod lr_apply;
    mod mc;
    mod mem;
    mod msac;
    mod obu;
    mod pal;
    mod picture;
    mod qm;
    mod recon;
    mod refmvs;
    mod scan;
    mod tables;
    mod thread_task;
    mod warpmv;
    mod wedge;
} // mod src

pub use src::error::Dav1dResult;
