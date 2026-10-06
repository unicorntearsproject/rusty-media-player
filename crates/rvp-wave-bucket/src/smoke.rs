//! A check of the thread start-up contract against a real engine (`cargo xtask bucket` runs it in Node): jobs run on threads the
//! app started with `thread_spawn`, each on its own stack and with its own thread-local block, while the main thread uses its own
//! stack at the same time. A stack or TLS that overlapped another thread's would show as a wrong result.
use std::cell::Cell;
use std::sync::atomic::{AtomicU32, Ordering};

static DONE: AtomicU32 = AtomicU32::new(0);
static GOOD: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static TLS: Cell<u32> = const { Cell::new(0) };
}

/// Recursion with a 2 KiB array per level: about 200 KiB of stack at depth 100.
#[inline(never)]
fn burn(depth: u32, seed: u32) -> u32 {
    let mut a = [0u8; 2048];
    for (i, b) in a.iter_mut().enumerate() {
        *b = (seed as usize).wrapping_mul(31).wrapping_add(i) as u8;
    }
    let here = a.iter().fold(0u32, |h, b| h.rotate_left(5) ^ u32::from(*b));
    if depth == 0 {
        here
    } else {
        burn(depth - 1, seed) ^ here.wrapping_add(std::hint::black_box(a[7]) as u32)
    }
}

fn job(id: u32, expected: u32) {
    TLS.with(|t| t.set(id + 1000));
    let mut ok = true;
    for round in 0..40 {
        ok &= burn(100, id) == expected;
        // Allocation and formatting on this thread, and the thread-local must still be ours.
        let s = format!("thread {id} round {round}");
        ok &= s.len() > 8 && TLS.with(Cell::get) == id + 1000;
    }
    if ok {
        GOOD.fetch_add(1, Ordering::AcqRel);
    }
    DONE.fetch_add(1, Ordering::AcqRel);
}

/// Start `n` threads that each run [`job`] while the main thread keeps using its stack; returns how many threads got the right
/// answers every time (and `-1` if the main thread's own results were disturbed, `-2` on a timeout).
#[unsafe(no_mangle)]
pub extern "C" fn bucket_smoke_threads(n: i32) -> i32 {
    rvp_host_rb::threads::init_main_thread();
    let started = rvp_host_rb::threads::init(4, 64);
    if started == 0 {
        return -3;
    }
    TLS.with(|t| t.set(7));
    let expected: Vec<u32> = (0..n as u32).map(|id| burn(100, id)).collect();
    for (id, want) in expected.iter().enumerate() {
        let (id, want) = (id as u32, *want);
        rvp_par::spawn(move || job(id, want));
    }
    let mut disturbed = false;
    // `std::time` does not exist on this target: time comes from the OS.
    let start = rvp_host_rb::api::now_us();
    loop {
        // The main thread works on its stack at the same time.
        for (id, want) in expected.iter().enumerate() {
            disturbed |= burn(100, id as u32) != *want;
        }
        disturbed |= TLS.with(Cell::get) != 7;
        if DONE.load(Ordering::Acquire) >= n as u32 {
            break;
        }
        if rvp_host_rb::api::now_us() - start > 20_000_000 {
            return -2;
        }
    }
    if disturbed { -1 } else { GOOD.load(Ordering::Acquire) as i32 }
}
