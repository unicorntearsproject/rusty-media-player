//! The subtitle parsers (SRT, WebVTT, ASS, container payloads, PGS display sets) on arbitrary bytes, and the cue list on
//! whatever they produce.
#![no_main]
use libfuzzer_sys::fuzz_target;
use rvp_subs::pgs::PgsDecoder;
use rvp_subs::{AssScript, CueList, decode_mkv_text, decode_mov_text, decode_wvtt_sample, parse, parse_srt, parse_vtt};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let mut cues = parse(&text);
    cues.extend(parse_srt(&text));
    cues.extend(parse_vtt(&text));
    cues.extend(AssScript::parse_file(&text));
    let list = CueList::from_cues(cues);
    for t in [i64::MIN / 2, -1, 0, 1_000, 3_600_000_000, i64::MAX / 2] {
        let _ = list.text_at(t);
        let _ = list.cues_at(t);
    }
    // The same bytes as an ASS header with a Matroska block, and as a run of PGS display sets.
    let script = AssScript::parse_header(&text);
    let _ = script.cue_from_block(data, 0, 1_000_000);
    let mut pgs = PgsDecoder::new();
    for chunk in data.chunks(97) {
        let _ = pgs.decode(chunk);
    }
    let _ = pgs.decode(data);
    let _ = decode_mov_text(data);
    let _ = decode_wvtt_sample(data);
    let _ = decode_mkv_text(data);
});
