//! Threads, for the threads build (`+atomics`, shared memory imported as `env.memory`).
//!
//! The App API leaves the start-up of a thread to the app: every thread is a **new instance** of the module on the same memory,
//! so the stack pointer and the thread-local block are per instance and the host promises nothing about them. So:
//!
//! 1. The app allocates the new thread's stack and TLS block and puts their addresses, with the job, in a small block whose
//!    address is the `arg` of `thread_spawn`.
//! 2. The OS runs the export `bucket_thread_start(tid, arg)` on the new thread. The first thing it does, with no stack use at all
//!    (a new instance starts at the *same* initial stack pointer as the main thread, whose frames live there), is point the
//!    stack pointer at the new stack; then it initialises TLS (`__wasm_init_tls`) and runs the job.
//! 3. The main thread initialises its own TLS block the same way at the start of `bucket_main`.
//!
//! In a build without atomics there are no threads: [`init`] returns 0 and the player stays single-threaded.

/// What the entry points of the module need to know.
#[cfg(not(all(target_arch = "wasm32", target_feature = "atomics")))]
mod imp {
    /// No threads in this build.
    pub fn init_main_thread() {}

    /// No threads in this build.
    pub fn init(_cpu_count: usize, _max_threads: usize) -> usize {
        0
    }

    /// No threads in this build: nothing runs.
    pub fn thread_entry(_tid: i32, _arg: i32) {}
}

#[cfg(all(target_arch = "wasm32", target_feature = "atomics"))]
mod imp {
    use crate::api;
    use bucket_v0_sys as sys;
    use std::alloc::{Layout, alloc, alloc_zeroed};
    use std::arch::asm;

    /// Stack of each thread, bytes (the codecs recurse a little; 2 MiB is what a native thread gets).
    const STACK: usize = 2 << 20;

    unsafe extern "C" {
        /// Synthesised by the linker for shared-memory builds: copies the TLS initial image into `mem` and makes it the thread's TLS.
        fn __wasm_init_tls(mem: *mut u8);
    }

    fn tls_size() -> usize {
        let v: usize;
        // SAFETY: reads a linker-defined constant global.
        unsafe {
            asm!(".globaltype __tls_size, i32, immutable", "global.get __tls_size", "local.set {0}", out(local) v, options(nomem, nostack, preserves_flags))
        };
        v
    }

    fn tls_align() -> usize {
        let v: usize;
        // SAFETY: reads a linker-defined constant global.
        unsafe {
            asm!(".globaltype __tls_align, i32, immutable", "global.get __tls_align", "local.set {0}", out(local) v, options(nomem, nostack, preserves_flags))
        };
        v
    }

    /// A zeroed TLS block for one thread (it lives as long as the thread, which is as long as the app).
    fn alloc_tls() -> *mut u8 {
        let (size, align) = (tls_size().max(1), tls_align().max(16));
        // SAFETY: a valid non-zero layout.
        unsafe { alloc_zeroed(Layout::from_size_align(size, align).unwrap_or(Layout::new::<u128>())) }
    }

    /// The main thread's own TLS block (the module's start function does not make one). Once only.
    pub fn init_main_thread() {
        static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if DONE.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return;
        }
        let tls = alloc_tls();
        // SAFETY: `tls` is a fresh block of the size the linker asked for.
        unsafe { __wasm_init_tls(tls) };
    }

    /// What `arg` points to. The stack top is first so the entry can read it without any other work.
    #[repr(C)]
    struct Start {
        stack_top: usize,
        tls: usize,
        job: Option<Box<dyn FnOnce() + Send>>,
    }

    fn spawn_on_bucket(job: Box<dyn FnOnce() + Send + 'static>) {
        // SAFETY: a valid layout; the stack is never freed (the thread lives as long as the app).
        let stack = unsafe { alloc(Layout::from_size_align(STACK, 16).unwrap_or(Layout::new::<u128>())) };
        if stack.is_null() {
            panic!("out of memory for a thread stack");
        }
        let start =
            Box::new(Start { stack_top: stack as usize + STACK, tls: alloc_tls() as usize, job: Some(job) });
        let arg = Box::into_raw(start);
        // SAFETY: no pointers; the block is the argument the new thread reads.
        let tid = unsafe { sys::thread_spawn(arg as usize as i32) };
        if tid < 0 {
            // SAFETY: the thread did not start, so the block is still ours.
            drop(unsafe { Box::from_raw(arg) });
            panic!("thread_spawn failed: {}", api::code_name(tid));
        }
    }

    /// The first code of a new thread. It must not touch the shadow stack before the stack pointer is switched, so it only reads
    /// the stack top out of the block and sets the global; everything else runs in [`thread_main`] on the new stack.
    ///
    /// Build with optimisations (the release profile): at `opt-level = 0` a function may spill to the shadow stack.
    #[inline(never)]
    pub fn thread_entry(tid: i32, arg: i32) {
        let top = {
            // SAFETY: `arg` is the block `spawn_on_bucket` made.
            unsafe { *(arg as usize as *const usize) }
        };
        // SAFETY: points the stack pointer at the memory this thread owns.
        unsafe {
            asm!("local.get {0}", "global.set __stack_pointer", in(local) top, options(nomem, preserves_flags))
        };
        thread_main(tid, arg);
    }

    #[inline(never)]
    fn thread_main(_tid: i32, arg: i32) {
        // SAFETY: the block is ours from here on (the spawner made it and does not touch it again).
        let mut start = unsafe { Box::from_raw(arg as usize as *mut Start) };
        // SAFETY: a TLS block of the size the linker asked for, used by this thread only.
        unsafe { __wasm_init_tls(start.tls as *mut u8) };
        if let Some(job) = start.job.take() {
            job();
        }
    }

    /// Turn threads on: the spawner, and the pool the pixel kernels share. Returns the pool size (0 if there are no threads).
    pub fn init(cpu_count: usize, max_threads: usize) -> usize {
        rvp_par::set_spawner(spawn_on_bucket);
        // The page's main thread draws every frame: it must not wait for workers.
        rvp_par::mark_ui_thread();
        if !rvp_par::available() {
            return 0;
        }
        // Leave room under the OS limit for the decoder threads the player starts on its own.
        let size = rvp_par::pool_size(cpu_count).min(max_threads.saturating_sub(4).max(1));
        rvp_par::Pool::new(size).install();
        size
    }
}

pub use imp::{init, init_main_thread, thread_entry};
