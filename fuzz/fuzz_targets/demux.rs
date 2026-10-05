//! MP4 and Matroska demuxing of arbitrary bytes: opening, reading every packet, metadata and a few seeks must never
//! panic, loop forever or allocate without bound.
#![no_main]
use libfuzzer_sys::fuzz_target;
use rvp_core::task::block_on;
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;

fuzz_target!(|data: &[u8]| {
    block_on(async {
        let Ok(mut d) = open(MemSource::new(data.to_vec())).await else { return };
        let _ = (d.streams().len(), d.duration_us(), d.metadata().title.is_some(), d.chapters().len());
        let mut n = 0;
        loop {
            match d.next_packet().await {
                Ok(Some(p)) => {
                    assert!(p.data.len() <= data.len() * 2 + 64, "a packet larger than the file");
                    n += 1;
                    if n > 20_000 {
                        break;
                    }
                }
                Ok(None) | Err(_) => break,
            }
        }
        for t in [0i64, 1_000_000, 123_456_789, i64::MAX / 4] {
            if d.seek(t).await.is_ok() {
                let _ = d.next_packet().await;
            }
        }
        // The packets of other streams before a point (the subtitle cues that began before a seek landed), whatever the ids.
        let ids: Vec<u32> = d.streams().iter().map(|s| s.id).chain([0, 1, 2, u32::MAX]).collect();
        for (a, b) in [(0i64, 5_000_000i64), (-1_000_000, 1_000), (123_456_789, 120_000_000), (i64::MIN / 4, i64::MAX / 4)] {
            if let Ok(v) = d.side_packets(&ids, a, b).await {
                assert!(v.iter().all(|p| p.pts >= a && p.pts <= b), "a packet outside the asked range");
            }
        }
    });
});
