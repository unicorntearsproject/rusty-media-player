//! The pipelined H.264 decoder (reconstruction and parsing on their own threads) on arbitrary Annex B bytes: it must
//! agree with the inline decoder on the number of frames and the frames themselves.
#![no_main]
use libfuzzer_sys::fuzz_target;
use rvp_codec_h264::decoder::Decoder;
fuzz_target!(|data: &[u8]| {
    // Inline reference.
    let mut inline = Decoder::new();
    inline.set_max_mbs(2048);
    let _ = inline.decode_annexb(data, 0);
    let _ = inline.flush();
    let mut want = Vec::new();
    while let Some(f) = inline.next_frame() {
        want.push(f);
    }
    // Pipelined: the same entry point, with the reconstruction and two parse threads.
    let mut dec = Decoder::new();
    dec.set_max_mbs(2048);
    dec.set_recon_executor(Box::new(rvp_par::h264::ThreadedRecon::new()));
    dec.set_parse_runner(Box::new(rvp_par::h264::ParseWorkers::new(2)));
    let _ = dec.decode_annexb(data, 0);
    let _ = dec.flush();
    let mut got = Vec::new();
    while let Some(f) = dec.next_frame() {
        got.push(f);
    }
    assert_eq!(want.len(), got.len(), "frame counts differ");
    for (a, b) in want.iter().zip(&got) {
        assert!(a == b, "frames differ");
    }
});
