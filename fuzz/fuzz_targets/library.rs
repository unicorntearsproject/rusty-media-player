//! The library's saved data and cover pictures on arbitrary bytes: the index, the playlists, a thumbnail, and JPEG/PNG decoding
//! (what a hostile file's embedded art or a damaged storage entry can reach).
#![no_main]
use libfuzzer_sys::fuzz_target;
use rvp_library::{Library, art, decode_thumb, encode_thumb};

fuzz_target!(|data: &[u8]| {
    if let Ok(mut l) = Library::load_index(data) {
        // Whatever loads must save and load again as the same thing.
        let bytes = l.save_index();
        let again = Library::load_index(&bytes).expect("what we save, we can load");
        assert_eq!(again.all_tracks().len(), l.all_tracks().len());
        let _ = l.search("a b");
        let _ = l.sorted_tracks(rvp_library::TrackSort::Album, true);
        let _ = l.load_playlists(data);
    }
    let mut l = Library::new();
    let _ = l.load_playlists(data);
    if let Some(t) = decode_thumb(data) {
        assert_eq!(decode_thumb(&encode_thumb(&t)), Some(t));
    }
    let _ = art::dimensions(data);
    let _ = art::decode(data, 64);
    // Playlist text through the library's resolver.
    let text = String::from_utf8_lossy(data);
    let id = l.import_playlist("fuzz", &text, "some/dir/list.m3u");
    for fmt in [rvp_library::ListFormat::M3u8, rvp_library::ListFormat::Pls] {
        let _ = l.export_playlist(id, fmt);
    }
});
