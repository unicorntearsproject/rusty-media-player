//! The text subtitle parsers (SRT, WebVTT, container payloads) on arbitrary bytes, and the cue list on whatever they
//! produce.
#![no_main]
use libfuzzer_sys::fuzz_target;
use rvp_subs::{CueList, decode_mkv_text, decode_mov_text, decode_wvtt_sample, parse, parse_srt, parse_vtt};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let mut cues = parse(&text);
    cues.extend(parse_srt(&text));
    cues.extend(parse_vtt(&text));
    let list = CueList::from_cues(cues);
    for t in [i64::MIN / 2, -1, 0, 1_000, 3_600_000_000, i64::MAX / 2] {
        let _ = list.text_at(t);
    }
    let _ = decode_mov_text(data);
    let _ = decode_wvtt_sample(data);
    let _ = decode_mkv_text(data);
});
