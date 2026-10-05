//! Inputs the fuzzer (`fuzz/fuzz_targets/subs.rs`) once crashed on. Same driver as the fuzz target.
use rvp_subs::pgs::PgsDecoder;
use rvp_subs::{AssScript, CueList, decode_mkv_text, decode_mov_text, decode_wvtt_sample, parse, parse_srt, parse_vtt};

#[test]
fn fuzz_regressions_do_not_panic() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fuzz_regressions");
    let mut n = 0;
    for e in std::fs::read_dir(dir).unwrap() {
        let data = std::fs::read(e.unwrap().path()).unwrap();
        let text = String::from_utf8_lossy(&data);
        let mut cues = parse(&text);
        cues.extend(parse_srt(&text));
        cues.extend(parse_vtt(&text));
        cues.extend(AssScript::parse_file(&text));
        let list = CueList::from_cues(cues);
        for t in [i64::MIN / 2, -1, 0, 1_000, 3_600_000_000, i64::MAX / 2] {
            let _ = list.text_at(t);
            let _ = list.cues_at(t);
        }
        let script = AssScript::parse_header(&text);
        let _ = script.cue_from_block(&data, 0, 1_000_000);
        let mut pgs = PgsDecoder::new();
        for chunk in data.chunks(97) {
            let _ = pgs.decode(chunk);
        }
        let _ = pgs.decode(&data);
        let _ = (decode_mov_text(&data), decode_wvtt_sample(&data), decode_mkv_text(&data));
        n += 1;
    }
    assert!(n > 0);
}
