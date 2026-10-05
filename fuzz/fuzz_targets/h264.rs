//! The H.264 decoder on arbitrary bytes, as an Annex B stream or as length-prefixed samples (first byte picks): no
//! panic, bounded memory, and `reset` and `flush` stay usable.
#![no_main]
use libfuzzer_sys::fuzz_target;
use rvp_codec_h264::decoder::Decoder;

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else { return };
    let mut dec = Decoder::new();
    // Small pictures keep the fuzzer fast and make a size claim of gigapixels hit the limit.
    dec.set_max_mbs(2048);
    if mode & 1 == 0 {
        for chunk in rest.chunks(rest.len().div_ceil(3).max(1)) {
            let _ = dec.decode_annexb(chunk, 0);
            while dec.next_frame().is_some() {}
        }
    } else {
        // Samples: each starts with a one-byte length-size selector, then NAL units with that prefix.
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
});
