//! Inputs the fuzzer (`fuzz/fuzz_targets/h264_pipelined.rs`) once found a difference or a crash on: the pipelined
//! decoder must produce exactly the frames of the inline decoder.
use rvp_codec_h264::decoder::Decoder;

#[test]
fn pipelined_equals_inline_on_regressions() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fuzz_regressions");
    let mut n = 0;
    for e in std::fs::read_dir(dir).unwrap() {
        let path = e.unwrap().path();
        let data = std::fs::read(&path).unwrap();
        let mut inline = Decoder::new();
        inline.set_max_mbs(2048);
        let _ = inline.decode_annexb(&data, 0);
        let _ = inline.flush();
        let mut want = Vec::new();
        while let Some(f) = inline.next_frame() {
            want.push(f);
        }
        for threads in [0usize, 1, 2, 3] {
            let mut dec = Decoder::new();
            dec.set_max_mbs(2048);
            dec.set_recon_executor(Box::new(rvp_par::h264::ThreadedRecon::new()));
            if threads > 0 {
                dec.set_parse_runner(Box::new(rvp_par::h264::ParseWorkers::new(threads)));
            }
            let _ = dec.decode_annexb(&data, 0);
            let _ = dec.flush();
            let mut got = Vec::new();
            while let Some(f) = dec.next_frame() {
                got.push(f);
            }
            assert_eq!(
                want.len(),
                got.len(),
                "{}: frame counts differ with {threads} parse threads",
                path.display()
            );
            for (i, (a, b)) in want.iter().zip(&got).enumerate() {
                assert!(a == b, "{}: frame {i} differs with {threads} parse threads", path.display());
            }
        }
        n += 1;
    }
    assert!(n > 0);
}
