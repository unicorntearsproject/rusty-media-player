//! The function set is the one the App API pages (draft v0.3) document: `app-api-reference.md` (System, Threads, Canvas, Theme),
//! `app-api-events.md` (Waiting), `app-api-media.md` (Video, Audio, Now playing, Visualizer, Library) and `app-api-files.md`
//! (Key-value store, Files). Written out by hand from those tables, so a change on either side shows up here.
#![cfg(feature = "inspect")]

use bucket_v0_sys::inspect::{ImportKind, ModuleInfo, check_imports, parse_module};
use bucket_v0_sys::{FUNCTIONS, MODULE};

const DOCUMENTED: &[&str] = &[
    // System
    "api_version",
    "caps",
    "launch_reason",
    "restart",
    "exit",
    "log",
    "random_fill",
    "time_now_us",
    "cpu_count",
    "limit_get",
    "power_inhibit",
    "clipboard_set_text",
    // Threads
    "thread_spawn",
    "thread_yield",
    "thread_priority",
    // Events
    "events_wait",
    "events_wake",
    "event_text",
    // Canvas
    "canvas_info",
    "canvas_present",
    "canvas_fullscreen",
    "frame_request",
    "cursor_set",
    "pointer_capture",
    // Theme and the Bucket Bar
    "theme_get",
    "bar_command",
    // Video
    "video_present",
    // Audio
    "audio_open",
    "audio_write",
    "audio_clock",
    "audio_queued",
    "audio_latency_us",
    "audio_flush",
    "audio_pause",
    "audio_volume",
    "audio_close",
    // Now playing
    "now_playing_metadata",
    "now_playing_playback",
    "now_playing_clear",
    // Visualizer
    "viz_block",
    "viz_summary",
    "viz_summary_n",
    // Library
    "library_add_folder",
    "library_roots",
    "library_reconnect",
    "library_rescan",
    "library_forget",
    "library_listing",
    "library_listing_release",
    // Key-value store
    "kv_load",
    "kv_store",
    "kv_flush",
    // Files
    "file_pick",
    "file_save",
    "file_open_id",
    "file_open_sibling",
    "file_open_bundle",
    "file_size",
    "file_read_at",
    "file_prefetch",
    "file_name",
    "file_id",
    "file_close",
];

#[test]
fn declared_set_is_the_documented_set() {
    let declared: Vec<&str> = FUNCTIONS.iter().map(|f| f.name).collect();
    assert_eq!(declared, DOCUMENTED);
}

// ---- the reader and the checker, on a module assembled here ----

fn leb(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn name(s: &str, out: &mut Vec<u8>) {
    leb(s.len() as u64, out);
    out.extend_from_slice(s.as_bytes());
}

fn section(id: u8, body: Vec<u8>, out: &mut Vec<u8>) {
    out.push(id);
    leb(body.len() as u64, out);
    out.extend(body);
}

fn val(t: &str) -> u8 {
    match t {
        "i32" => 0x7f,
        "i64" => 0x7e,
        "f32" => 0x7d,
        other => panic!("{other}"),
    }
}

/// A module that imports exactly `funcs` (module, name, params, results) and optionally `env.memory`, exporting `bucket_main`.
fn module(funcs: &[(&str, &str, Vec<&str>, Vec<&str>)], shared_memory: bool) -> Vec<u8> {
    let mut m = b"\0asm\x01\0\0\0".to_vec();
    let mut types = Vec::new();
    leb(funcs.len() as u64, &mut types);
    for (_, _, p, r) in funcs {
        types.push(0x60);
        leb(p.len() as u64, &mut types);
        types.extend(p.iter().map(|t| val(t)));
        leb(r.len() as u64, &mut types);
        types.extend(r.iter().map(|t| val(t)));
    }
    section(1, types, &mut m);
    let mut imports = Vec::new();
    leb(funcs.len() as u64 + shared_memory as u64, &mut imports);
    if shared_memory {
        name("env", &mut imports);
        name("memory", &mut imports);
        imports.push(2);
        imports.push(0x03); // max present, shared
        leb(17, &mut imports);
        leb(32768, &mut imports);
    }
    for (i, (module, f, _, _)) in funcs.iter().enumerate() {
        name(module, &mut imports);
        name(f, &mut imports);
        imports.push(0);
        leb(i as u64, &mut imports);
    }
    section(2, imports, &mut m);
    let mut exports = Vec::new();
    leb(1, &mut exports);
    name("bucket_main", &mut exports);
    exports.push(0);
    leb(funcs.len() as u64, &mut exports);
    section(7, exports, &mut m);
    // A custom section must be skipped.
    let mut custom = Vec::new();
    name("producers", &mut custom);
    section(0, custom, &mut m);
    m
}

fn all_documented() -> Vec<(&'static str, &'static str, Vec<&'static str>, Vec<&'static str>)> {
    FUNCTIONS
        .iter()
        .map(|f| {
            (MODULE, f.name, f.params.to_vec(), if f.result.is_empty() { vec![] } else { vec![f.result] })
        })
        .collect()
}

#[test]
fn a_module_with_exactly_the_documented_imports_passes() {
    let info = parse_module(&module(&all_documented(), true)).unwrap();
    assert_eq!(info.imports.len(), FUNCTIONS.len() + 1);
    assert!(matches!(info.imports[0].kind, ImportKind::Memory { shared: true, min: 17, max: Some(32768) }));
    assert_eq!(info.exports, vec![("bucket_main".to_string(), 0)]);
    let rep = check_imports(&info);
    assert!(rep.ok(), "{:?}", rep.problems);
    assert!(rep.shared_memory);
    assert_eq!(rep.bucket_functions, FUNCTIONS.len());
}

#[test]
fn drift_is_reported() {
    // A missing function, a wrong signature, an undocumented name and an import from another module.
    let mut f = all_documented();
    f.retain(|x| x.1 != "kv_flush");
    f.iter_mut().find(|x| x.1 == "kv_load").unwrap().3 = vec!["i64"];
    f.push((MODULE, "kv_loadd", vec!["i32"], vec!["i32"]));
    f.push(("env", "abort", vec![], vec![]));
    let rep = check_imports(&parse_module(&module(&f, false)).unwrap());
    assert!(!rep.ok());
    let all = rep.problems.join("\n");
    assert!(all.contains("`bucket_v0.kv_flush` is not imported"), "{all}");
    assert!(all.contains("`bucket_v0.kv_load` is imported as"), "{all}");
    assert!(all.contains("`bucket_v0.kv_loadd` is not a documented"), "{all}");
    assert!(all.contains("unexpected import `env.abort`"), "{all}");
    assert_eq!(rep.problems.len(), 4, "{all}");
}

#[test]
fn garbage_is_an_error_not_a_panic() {
    assert!(parse_module(b"hello").is_err());
    assert!(parse_module(b"\0asm\x02\0\0\0").is_err());
    let good = module(&all_documented(), false);
    for cut in [9, 20, good.len() / 2, good.len() - 1] {
        let _ = parse_module(&good[..cut]); // must not panic
    }
    let info: ModuleInfo = parse_module(b"\0asm\x01\0\0\0").unwrap();
    assert!(info.imports.is_empty());
}
