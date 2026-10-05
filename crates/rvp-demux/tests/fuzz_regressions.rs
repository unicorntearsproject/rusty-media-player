//! Inputs the fuzzer (`fuzz/fuzz_targets/demux.rs`) once crashed or exhausted memory on. Each file in
//! `tests/fuzz_regressions` must open and read to the end (or fail cleanly) without panicking.
use rvp_core::task::block_on;
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;

#[test]
fn fuzz_regressions_do_not_panic() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fuzz_regressions");
    let mut n = 0;
    for e in std::fs::read_dir(dir).unwrap() {
        let path = e.unwrap().path();
        let data = std::fs::read(&path).unwrap();
        block_on(async {
            let Ok(mut d) = open(MemSource::new(data.clone())).await else { return };
            let _ = (d.streams().len(), d.duration_us(), d.chapters().len());
            let mut packets = 0;
            while let Ok(Some(p)) = d.next_packet().await {
                assert!(
                    p.data.len() <= data.len() * 2 + 64,
                    "{}: a packet larger than the file",
                    path.display()
                );
                packets += 1;
                if packets > 20_000 {
                    break;
                }
            }
            for t in [0i64, 1_000_000, 123_456_789, i64::MAX / 4] {
                if d.seek(t).await.is_ok() {
                    let _ = d.next_packet().await;
                }
            }
        });
        n += 1;
    }
    assert!(n > 0, "no regression inputs");
}
