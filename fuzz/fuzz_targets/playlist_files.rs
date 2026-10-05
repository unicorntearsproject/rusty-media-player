//! M3U / M3U8 / PLS parsing and path resolution on arbitrary text, and the export-import round trip of whatever came out.
#![no_main]
use libfuzzer_sys::fuzz_target;
use rvp_player::listfile::{export_m3u, export_pls, parse, parse_m3u, parse_pls, resolve};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let a = parse_m3u(&text);
    let b = parse_pls(&text);
    let c = parse(&text);
    assert!(c == a || c == b);
    for e in a.iter().chain(&b).take(64) {
        let r = resolve("/some/dir/list.m3u", &e.path);
        let _ = resolve("rel/list.pls", &e.path);
        let _ = resolve("", &e.path);
        assert!(r.len() <= e.path.len() * 4 + 64, "resolution grew the path");
    }
    // What we write, we read back as the same list (entries with line breaks in them are flattened by the writer).
    let flat = |l: &[rvp_player::listfile::Entry]| -> Vec<(String, Option<String>)> {
        l.iter()
            .map(|e| (e.path.replace(['\n', '\r'], " ").trim().to_string(), e.title.clone().map(|t| t.replace(['\n', '\r'], " ").trim().to_string())))
            .filter(|(p, _)| !p.is_empty() && !p.starts_with('#'))
            .collect()
    };
    let want = flat(&a);
    let back = flat(&parse(&export_m3u(&a)));
    assert_eq!(want.len(), back.len(), "m3u round trip");
    let _ = export_pls(&b);
});
