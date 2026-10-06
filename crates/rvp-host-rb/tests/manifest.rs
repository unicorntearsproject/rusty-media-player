//! The manifest template says what the adapter does: the file types match the picker, the optional capabilities are real ones, the
//! app ID is the product's, and the builds are left to `cargo xtask bucket`.
use bucket_v0_sys::caps;
use rvp_host_rb::OPEN_EXTENSIONS;

fn manifest() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/bucket/manifest.toml.in"))
        .expect("the manifest template")
}

/// The quoted strings of the array that follows `key =` (the array may span lines).
fn array(text: &str, key: &str) -> Vec<String> {
    let start = text.find(&format!("\n{key} = [")).unwrap_or_else(|| panic!("no `{key}`")) + key.len() + 5;
    let end = start + text[start..].find(']').expect("closing bracket");
    text[start..end].split('"').skip(1).step_by(2).map(String::from).collect()
}

fn value(text: &str, key: &str) -> String {
    let line =
        text.lines().find(|l| l.starts_with(&format!("{key} = "))).unwrap_or_else(|| panic!("no `{key}`"));
    line.split_once('=').unwrap().1.split('#').next().unwrap().trim().trim_matches('"').to_string()
}

#[test]
fn identity_and_policy() {
    let m = manifest();
    assert_eq!(value(&m, "id"), "io.github.idometeor.RustyWave");
    assert_eq!(value(&m, "name"), "Rusty Wave");
    assert_eq!(value(&m, "api"), "bucket_v0");
    assert_eq!(value(&m, "api_min"), "0.3");
    assert_eq!(value(&m, "version"), "@VERSION@");
    assert_eq!(value(&m, "class"), "media");
    assert_eq!(value(&m, "restart"), "on-trap");
    assert_eq!(value(&m, "restart_limit"), "3/60s");
    assert_eq!(value(&m, "memory_max_mb"), "2048");
    assert!(m.contains("# @BUILDS@"), "the build step puts the [[builds]] here");
    assert!(!m.lines().any(|l| l.starts_with("[[builds]]")), "the builds come from what was built");
}

#[test]
fn only_the_canvas_is_required() {
    let m = manifest();
    assert_eq!(array(&m, "capabilities"), ["CANVAS"]);
    let known = [
        ("THREADS", caps::THREADS),
        ("SIMD128", caps::SIMD128),
        ("ATOMICS", caps::ATOMICS),
        ("CANVAS", caps::CANVAS),
        ("VIDEO_YUV", caps::VIDEO_YUV),
        ("AUDIO_OUT", caps::AUDIO_OUT),
        ("AUDIO_IN", caps::AUDIO_IN),
        ("NOW_PLAYING", caps::NOW_PLAYING),
        ("VISUALIZER", caps::VISUALIZER),
        ("LIBRARY", caps::LIBRARY),
        ("BAR_COMMANDS", caps::BAR_COMMANDS),
        ("THEME", caps::THEME),
        ("FRAME_EVENTS", caps::FRAME_EVENTS),
        ("CLIPBOARD", caps::CLIPBOARD),
    ];
    for name in array(&m, "optional") {
        assert!(known.iter().any(|(n, _)| *n == name), "`{name}` is not a capability");
    }
    // Sound and the shell are optional (video plays without a device); the app uses all of these when they are there.
    let optional = array(&m, "optional");
    for need in ["AUDIO_OUT", "NOW_PLAYING", "VISUALIZER", "LIBRARY", "VIDEO_YUV"] {
        assert!(optional.iter().any(|o| o == need), "{need}");
    }
}

#[test]
fn the_file_types_are_the_ones_the_picker_offers() {
    let m = manifest();
    let declared = array(&m, "extensions");
    assert_eq!(declared, OPEN_EXTENSIONS, "[[opens]] and the picker must list the same types");
    assert_eq!(value(&m, "role"), "view");
}
