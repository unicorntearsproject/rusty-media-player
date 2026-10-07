//! What the user reads when something goes wrong: every message says what happened, why, and what to do next, in the app's voice, and
//! names the codec or format. The text is built here and nowhere else, so it stays one voice and can be tested in one place.
use alloc::format;
use alloc::string::String;
use rvp_core::Error;

/// A codec or container name as people write it.
pub fn codec_name(codec: &str) -> String {
    match codec {
        "hevc" | "h265" => "HEVC (H.265)".into(),
        "h264" | "avc" | "avc1" => "H.264".into(),
        "av1" => "AV1".into(),
        "vp9" => "VP9".into(),
        "vp8" => "VP8".into(),
        "mpeg2video" | "mpeg2" => "MPEG-2".into(),
        "mpeg4" => "MPEG-4 Part 2".into(),
        "vc1" => "VC-1".into(),
        "prores" => "ProRes".into(),
        "aac" => "AAC".into(),
        "mp3" => "MP3".into(),
        "flac" => "FLAC".into(),
        "opus" => "Opus".into(),
        "vorbis" => "Vorbis".into(),
        "ac3" => "AC-3 (Dolby Digital)".into(),
        "eac3" => "E-AC-3 (Dolby Digital Plus)".into(),
        "dts" => "DTS".into(),
        "truehd" => "Dolby TrueHD".into(),
        "alac" => "Apple Lossless".into(),
        "wma" | "wmav2" => "WMA".into(),
        "" => "an unknown codec".into(),
        other => other.to_uppercase(),
    }
}

/// The codec a message of the session names, from `video disabled (hevc): video codec `hevc` [WebCodecs: why]`.
fn between(s: &str, open: char, close: char) -> Option<&str> {
    let a = s.find(open)? + open.len_utf8();
    let b = s[a..].find(close)? + a;
    Some(&s[a..b])
}

/// The reason a platform gave ("WebCodecs", "no HEVC decoder here") from `... [WebCodecs: no HEVC decoder here]`.
fn platform_reason(msg: &str) -> Option<(&str, &str)> {
    let inner = between(msg, '[', ']')?;
    inner.split_once(": ")
}

/// A sentence about a video with no picture: the codec, why, what to do. The sound goes on, so the message says so.
pub fn no_picture(warning: &str) -> String {
    // warning: `video disabled (hevc): video codec `hevc` [WebCodecs: no HEVC decoder here]`
    let codec = between(warning, '(', ')').unwrap_or("");
    let detail = warning.split_once("): ").map_or(warning, |(_, d)| d);
    let name = codec_name(codec);
    let tail = " The sound plays on.";
    if let Some((platform, why)) = platform_reason(detail) {
        return format!(
            "No picture: this video is {name}, and {platform} can't decode it here: {why}. Convert it to H.264 or AV1 to watch it here.{tail}"
        );
    }
    let d = detail.to_lowercase();
    if d.contains("bit depth above 8") || d.contains("10-bit") || d.contains("bit depth 10") {
        return format!(
            "No picture: this video is 10-bit {name}, which isn't supported yet. Re-encode it as 8-bit {name} (or AV1) to watch it.{tail}"
        );
    }
    if d.contains("interlaced") {
        return format!(
            "No picture: this video is interlaced, which isn't supported yet. De-interlace it when you convert it to H.264.{tail}"
        );
    }
    if d.contains("chroma format") || d.contains("4:2:2") || d.contains("4:4:4") {
        return format!(
            "No picture: this {name} video uses a colour layout (4:2:2 or 4:4:4) that isn't supported yet. Convert it to 4:2:0 to watch it.{tail}"
        );
    }
    if d.contains("picture too large") || d.contains("size limit") || d.contains("picture of") {
        return format!(
            "No picture: this video is bigger than Rusty Wave can decode. Scale it down (4K or smaller) to watch it.{tail}"
        );
    }
    if matches!(codec, "hevc" | "h265") {
        return format!(
            "No picture: this video is {name}, which this device can't decode here. Convert it to H.264 or AV1, or open it in the desktop app on a computer with HEVC support.{tail}"
        );
    }
    if d.contains("slice groups") || d.contains("sp/si") || d.contains("data partitioning") {
        return format!(
            "No picture: this {name} video uses an old coding tool (FMO, SP/SI or data partitioning) that isn't supported. Convert it to a modern H.264 file.{tail}"
        );
    }
    format!(
        "No picture: this video is {name}, which isn't supported yet. Convert it to H.264, VP9 or AV1 in an MP4, MKV or WebM file to watch it.{tail}"
    )
}

/// A sentence about a file with no sound.
pub fn no_sound(warning: &str) -> String {
    let codec = between(warning, '(', ')').unwrap_or("");
    let detail = warning.split_once("): ").map_or(warning, |(_, d)| d);
    let name = codec_name(codec);
    let d = detail.to_lowercase();
    let why = if d.contains("more than 2 channels") || d.contains("channels") {
        format!("this {name} audio has more channels than Rusty Wave plays (stereo or mono)")
    } else {
        format!("this file's audio is {name}, which isn't supported yet")
    };
    format!(
        "No sound: {why}. Convert the audio to AAC, MP3, FLAC, Opus or Vorbis to hear it. The picture plays on."
    )
}

/// An error that stopped a file from opening, in the app's voice.
pub fn friendly_error(e: &Error) -> String {
    match e {
        Error::Unsupported(m) => unsupported(m),
        Error::Truncated => {
            "This file ends too early, so it can't be played all the way. If you downloaded it, download it again; if it came from a drive or a share, check that the copy finished."
                .into()
        }
        Error::Invalid(m) => format!(
            "This file looks damaged ({m}). Try another copy of it; if it plays elsewhere, tell us which app and we'll look."
        ),
        Error::Host(m) => host_error(m),
    }
}

fn unsupported(m: &str) -> String {
    let lower = m.to_lowercase();
    if let Some(c) = between(m, '`', '`') {
        if lower.starts_with("video codec") {
            let name = codec_name(c);
            return match platform_reason(m) {
                Some((p, why)) => format!(
                    "This video is {name}, and {p} can't decode it here: {why}. Convert it to H.264, VP9 or AV1, or try the desktop app."
                ),
                None => format!(
                    "This video is {name}, which Rusty Wave can't decode here. Convert it to H.264, VP9 or AV1 in an MP4, MKV or WebM file."
                ),
            };
        }
        if lower.starts_with("audio codec") {
            return format!(
                "This audio is {}, which Rusty Wave can't play yet. Convert it to AAC, MP3, FLAC, Opus or Vorbis.",
                codec_name(c)
            );
        }
    }
    if lower.contains("no video stream") {
        return "There is no picture in this file to play. If it should have one, it may be damaged or in a format Rusty Wave doesn't read.".into();
    }
    if lower.contains("no audio stream") || lower.contains("no audio frames") {
        return "There is no sound in this file to play. If it should have some, it may be damaged or in a format Rusty Wave doesn't read.".into();
    }
    if lower.contains("not an mp4")
        || lower.contains("not a matroska")
        || lower.contains("unrecognised container")
    {
        return "Rusty Wave doesn't recognise this file. It plays MP4, MKV, WebM, MP3, FLAC, Ogg (Opus and Vorbis), WAV and AAC; convert it, or check that it isn't damaged or still downloading.".into();
    }
    if lower.contains("fragmented file with a random access index") {
        return "This MP4 is a fragmented file with an index Rusty Wave can't read yet. Re-save it with ffmpeg (`-movflags +faststart`) to play it.".into();
    }
    if lower.contains("bit depth") || lower.contains("lossless") {
        return "This video uses more than 8 bits per colour in a way Rusty Wave can't decode yet. Re-encode it as 8-bit H.264, VP9 or AV1.".into();
    }
    if lower.contains("interlaced") {
        return "This video is interlaced, which isn't supported yet. De-interlace it when you convert it to H.264.".into();
    }
    if lower.contains("chroma") || lower.contains("4:2:2") || lower.contains("4:4:4") {
        return "This video uses a colour layout (4:2:2 or 4:4:4) that isn't supported yet. Convert it to 4:2:0.".into();
    }
    if lower.contains("channels") {
        return "This sound has more channels than Rusty Wave plays (stereo or mono). Mix it down to stereo."
            .into();
    }
    if lower.contains("picture too large") || lower.contains("size limit") {
        return "This picture is bigger than Rusty Wave can decode. Scale the video down (4K or smaller)."
            .into();
    }
    format!("Rusty Wave can't play this yet ({m}). If you can, convert it to MP4 with H.264 and AAC.")
}

fn host_error(m: &str) -> String {
    let lower = m.to_lowercase();
    if lower.contains("permission") || lower.contains("denied") || lower.contains("not allowed") {
        return format!(
            "Rusty Wave isn't allowed to read this file ({m}). Check the file's permissions, or pick the folder again so access is granted."
        );
    }
    if lower.contains("not found") || lower.contains("no such file") {
        return format!(
            "This file isn't there any more ({m}). It may have been moved, renamed or deleted, or its drive isn't connected."
        );
    }
    if lower.contains("no space")
        || lower.contains("quota")
        || lower.contains("storage full")
        || lower.contains("disk full")
    {
        return format!(
            "There's no room left to save that ({m}). Free some space on this device, then try again."
        );
    }
    if lower.contains("network")
        || lower.contains("offline")
        || lower.contains("timed out")
        || lower.contains("connection")
    {
        return format!("The network let us down ({m}). Check your connection and try again.");
    }
    format!(
        "Couldn't read the file ({m}). Check that it's still there and that you can open it in another app, then try again."
    )
}

/// A toast for a file that would not open: its name, then what happened, why and what to do.
pub fn couldnt_open(name: &str, e: &Error) -> String {
    if name.is_empty() {
        format!("Couldn't open that. {}", friendly_error(e))
    } else {
        format!("Couldn't open {name}. {}", friendly_error(e))
    }
}

/// A queue item whose file the host can no longer reach by its saved id.
pub fn cannot_reopen() -> String {
    "Rusty Wave can't reopen that file: this host only opens files you have just picked. Pick it again (or add its folder to the library) to play it."
        .into()
}

/// A line for what failed while saving something of the user's (the library, a playlist, the settings).
pub fn save_failed(what: &str, why: &str) -> String {
    let lower = why.to_lowercase();
    if lower.contains("quota") || lower.contains("no space") || lower.contains("full") {
        return format!(
            "Couldn't save {what}: this device is out of storage. Free some space and try again."
        );
    }
    if lower.contains("permission") || lower.contains("denied") {
        return format!(
            "Couldn't save {what}: Rusty Wave isn't allowed to write there ({why}). Check the folder's permissions."
        );
    }
    format!(
        "Couldn't save {what} ({why}). Try again; if it keeps failing, check that the drive is connected and has room."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn a_video_without_a_picture_names_the_codec_the_reason_and_the_next_step() {
        let m = no_picture("video disabled (hevc): video codec `hevc`");
        assert!(
            m.contains("HEVC (H.265)") && m.contains("Convert it") && m.contains("sound plays on"),
            "{m}"
        );
        let m = no_picture(
            "video disabled (hevc): video codec `hevc` [WebCodecs: this browser has no HEVC decoder]",
        );
        assert!(m.contains("WebCodecs can't decode it here") && m.contains("no HEVC decoder"), "{m}");
        let m = no_picture("video disabled (h264): bit depth above 8 or lossless coding");
        assert!(m.contains("10-bit H.264") && m.contains("8-bit"), "{m}");
        let m = no_picture("video disabled (mpeg2video): video codec `mpeg2video`");
        assert!(m.contains("MPEG-2") && m.contains("H.264, VP9 or AV1"), "{m}");
    }

    #[test]
    fn a_file_without_sound_says_so_and_what_to_do() {
        let m = no_sound("audio disabled (ac3): audio codec `ac3`");
        assert!(
            m.contains("AC-3") && m.contains("Convert the audio") && m.contains("picture plays on"),
            "{m}"
        );
    }

    #[test]
    fn open_errors_say_what_why_and_next() {
        for (e, needles) in [
            (Error::Truncated, vec!["ends too early", "download it again"]),
            (Error::Invalid("bad box".into()), vec!["damaged", "bad box", "another copy"]),
            (Error::Host("permission denied".into()), vec!["isn't allowed", "permissions"]),
            (Error::Host("No such file or directory".into()), vec!["isn't there any more", "moved"]),
            (Error::Host("no space left on device".into()), vec!["no room left", "Free some space"]),
            (
                Error::Unsupported("unrecognised container".into()),
                vec!["doesn't recognise", "MP4, MKV, WebM"],
            ),
            (Error::Unsupported("video codec `hevc`".into()), vec!["HEVC (H.265)", "Convert it"]),
            (Error::Unsupported("audio codec `ac3`".into()), vec!["AC-3", "Convert it to AAC"]),
            (Error::Unsupported("no video stream".into()), vec!["no picture in this file"]),
        ] {
            let m = friendly_error(&e);
            for n in needles {
                assert!(m.contains(n), "{e:?} -> {m}");
            }
        }
    }
}
