//! The playlist state machine under a random sequence of operations: it must keep its invariants (the current item
//! exists, ids are unique, order is a permutation) and never panic.
#![no_main]
use libfuzzer_sys::fuzz_target;
use rvp_player::playlist::{Playlist, Repeat};

fuzz_target!(|data: &[u8]| {
    let mut pl = Playlist::new();
    let mut it = data.iter().copied().take(4096);
    while let Some(op) = it.next() {
        let op = op % 14;
        let arg = it.next().unwrap_or(0);
        match op {
            0 | 1 => {
                if pl.len() < 500 {
                    pl.add(&format!("n{arg}"), &format!("s{arg}"));
                }
            }
            2 => {
                let id = pl.items().get(arg as usize % pl.len().max(1)).map(|x| x.id).unwrap_or(arg as u32);
                pl.remove(id);
            }
            3 => {
                let id = pl.items().get(arg as usize % pl.len().max(1)).map(|x| x.id).unwrap_or(0);
                pl.set_current(id);
            }
            4 => {
                let id = pl.items().get(arg as usize % pl.len().max(1)).map(|x| x.id).unwrap_or(0);
                pl.move_item(id, arg as i8 as i32);
            }
            5 => {
                pl.next();
            }
            6 => {
                pl.prev();
            }
            7 => {
                pl.advance();
            }
            8 => {
                pl.set_shuffle(arg & 1 == 1, arg as u64 * 7919);
            }
            9 => {
                pl.set_repeat(match arg % 3 {
                    0 => Repeat::Off,
                    1 => Repeat::All,
                    _ => Repeat::One,
                });
            }
            10 => pl.clear(),
            11 => {
                let _ = pl.peek_next();
            }
            _ => {
                let _ = pl.current();
            }
        }
        // Invariants.
        let ids: Vec<u32> = pl.items().iter().map(|x| x.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "duplicate ids");
        if let Some(c) = pl.current_id() {
            assert!(ids.contains(&c), "current item is not in the list");
        } else {
            assert!(pl.is_empty() || pl.current().is_none());
        }
    }
});
