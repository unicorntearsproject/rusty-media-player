//! Inputs the fuzzer (`fuzz/fuzz_targets/h264.rs`) once crashed on. Same driver as the fuzz target.
use rvp_codec_h264::decoder::Decoder;

#[test]
fn fuzz_regressions_do_not_panic() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fuzz_regressions");
    let mut n = 0;
    for e in std::fs::read_dir(dir).unwrap() {
        let data = std::fs::read(e.unwrap().path()).unwrap();
        let Some((&mode, rest)) = data.split_first() else { continue };
        let mut dec = Decoder::new();
        dec.set_max_mbs(2048);
        if mode & 1 == 0 {
            for chunk in rest.chunks(rest.len().div_ceil(3).max(1)) {
                let _ = dec.decode_annexb(chunk, 0);
                while dec.next_frame().is_some() {}
            }
        } else {
            let mut pts = 0;
            for chunk in rest.chunks(rest.len().div_ceil(4).max(1)) {
                let _ = dec.decode_sample(chunk, pts);
                pts += 40_000;
                while dec.next_frame().is_some() {}
            }
        }
        if mode & 2 != 0 {
            dec.reset();
        }
        let _ = dec.flush();
        while dec.next_frame().is_some() {}
        n += 1;
    }
    assert!(n > 0);
}
