//! Inputs the fuzzer (`fuzz/fuzz_targets/subs.rs`) once crashed on. Same driver as the fuzz target.
use rvp_subs::{CueList, decode_mkv_text, decode_mov_text, decode_wvtt_sample, parse, parse_srt, parse_vtt};

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
        let list = CueList::from_cues(cues);
        for t in [i64::MIN / 2, -1, 0, 1_000, 3_600_000_000, i64::MAX / 2] {
            let _ = list.text_at(t);
        }
        let _ = (decode_mov_text(&data), decode_wvtt_sample(&data), decode_mkv_text(&data));
        n += 1;
    }
    assert!(n > 0);
}
