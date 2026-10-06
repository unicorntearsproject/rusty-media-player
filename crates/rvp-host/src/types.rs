//! The media types Rusty Wave plays, for the "set as default media player" checklist and for the hosts that register themselves with the
//! system (a desktop file's `MimeType`, the Windows file associations, a macOS handler). One table, so the checklist, the packages and
//! the manifest of Rusty Bucket cannot drift apart (a test in this crate compares it with the desktop file and the Windows installer).
//!
//! Subtitle files are not here: they are companions of a video, and taking `.srt` over from the editor that has it would be rude.
use alloc::string::String;
use alloc::vec::Vec;

/// One row of the checklist: a kind of file, with the extensions and media types the system knows it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaType {
    /// Short stable id (`mp4`, `flac`), what a host is told.
    pub id: &'static str,
    /// What the checklist says.
    pub label: &'static str,
    /// File name extensions without the dot, lower case.
    pub extensions: &'static [&'static str],
    /// Media types (MIME), most common first.
    pub mimes: &'static [&'static str],
    /// A video container (the rest are audio or playlists).
    pub video: bool,
}

/// Every media type the player opens, videos first.
pub const MEDIA_TYPES: &[MediaType] = &[
    MediaType {
        id: "mp4",
        label: "MP4 and M4V video",
        extensions: &["mp4", "m4v"],
        mimes: &["video/mp4", "video/x-m4v"],
        video: true,
    },
    MediaType {
        id: "mkv",
        label: "Matroska video (MKV)",
        extensions: &["mkv"],
        mimes: &["video/x-matroska"],
        video: true,
    },
    MediaType { id: "webm", label: "WebM video", extensions: &["webm"], mimes: &["video/webm"], video: true },
    MediaType {
        id: "mka",
        label: "Matroska audio (MKA)",
        extensions: &["mka"],
        mimes: &["audio/x-matroska"],
        video: false,
    },
    MediaType { id: "mp3", label: "MP3 audio", extensions: &["mp3"], mimes: &["audio/mpeg"], video: false },
    MediaType {
        id: "flac",
        label: "FLAC audio",
        extensions: &["flac"],
        mimes: &["audio/flac", "audio/x-flac"],
        video: false,
    },
    MediaType {
        id: "ogg",
        label: "Ogg Vorbis and Opus audio",
        extensions: &["ogg", "oga", "opus"],
        mimes: &["audio/ogg", "audio/x-vorbis+ogg", "audio/x-opus+ogg"],
        video: false,
    },
    MediaType {
        id: "wav",
        label: "WAV audio",
        extensions: &["wav"],
        mimes: &["audio/vnd.wave", "audio/x-wav", "audio/wav"],
        video: false,
    },
    MediaType {
        id: "aac",
        label: "AAC and M4A audio",
        extensions: &["m4a", "m4b", "aac"],
        mimes: &["audio/aac", "audio/mp4", "audio/x-m4b"],
        video: false,
    },
    MediaType {
        id: "playlist",
        label: "Playlists (M3U, M3U8, PLS)",
        extensions: &["m3u", "m3u8", "pls"],
        mimes: &["audio/x-mpegurl", "application/vnd.apple.mpegurl", "audio/x-scpls"],
        video: false,
    },
];

/// The types with these ids, in table order (unknown ids are ignored).
pub fn media_types_by_id(ids: &[&str]) -> Vec<&'static MediaType> {
    MEDIA_TYPES.iter().filter(|t| ids.contains(&t.id)).collect()
}

/// Every media type (MIME) of `types`.
pub fn mimes_of(types: &[&MediaType]) -> Vec<String> {
    types.iter().flat_map(|t| t.mimes.iter().map(|m| String::from(*m))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESKTOP: &str =
        include_str!("../../../packaging/shared/io.github.unicorntearsproject.RustyWave.desktop");
    const ISS: &str = include_str!("../../../packaging/windows/rusty-wave.iss");

    #[test]
    fn the_table_is_what_the_desktop_file_and_the_windows_installer_declare() {
        let mime_line = DESKTOP.lines().find(|l| l.starts_with("MimeType=")).expect("a MimeType line");
        let declared: Vec<&str> =
            mime_line["MimeType=".len()..].split(';').filter(|m| !m.is_empty()).collect();
        for t in MEDIA_TYPES {
            for m in t.mimes {
                assert!(declared.contains(m), "{m} is missing from the desktop file");
            }
            for e in t.extensions {
                assert!(
                    ISS.contains(&std::format!("ValueName: \".{e}\"")),
                    ".{e} is missing from the installer"
                );
            }
        }
        // And nothing else is declared but the two subtitle types.
        for m in declared {
            assert!(
                MEDIA_TYPES.iter().any(|t| t.mimes.contains(&m))
                    || m == "application/x-subrip"
                    || m == "text/vtt",
                "{m} is declared but not in the table"
            );
        }
    }

    #[test]
    fn ids_are_unique_and_lookups_keep_table_order() {
        let mut ids: Vec<_> = MEDIA_TYPES.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), MEDIA_TYPES.len());
        let got = media_types_by_id(&["flac", "mp4", "nope"]);
        assert_eq!(got.iter().map(|t| t.id).collect::<Vec<_>>(), ["mp4", "flac"]);
        assert!(mimes_of(&got).contains(&String::from("audio/x-flac")));
        assert!(MEDIA_TYPES.iter().take_while(|t| t.video).count() >= 3, "videos come first");
    }
}
