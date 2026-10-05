//! Media descriptors: streams, packets, decoded video frames and audio buffers.
use crate::time::{Rational, Timestamp};
use alloc::{string::String, vec::Vec};

/// Kind of an elementary stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    /// Video.
    Video,
    /// Audio.
    Audio,
    /// Text subtitles.
    Subtitle,
}

/// Video codecs the player knows about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    /// H.264 / AVC.
    H264,
    /// AV1.
    Av1,
    /// VP9.
    Vp9,
}

/// Audio codecs the player knows about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    /// AAC (LC, HE).
    Aac,
    /// MP3.
    Mp3,
    /// FLAC.
    Flac,
    /// Opus.
    Opus,
    /// Vorbis.
    Vorbis,
    /// Uncompressed PCM (WAV).
    Pcm,
}

/// Video-specific stream parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoInfo {
    /// Coded width in pixels.
    pub width: u32,
    /// Coded height in pixels.
    pub height: u32,
}

/// Audio-specific stream parameters, as declared by the container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioInfo {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u16,
}

/// Description of one elementary stream, as reported by a demuxer.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamInfo {
    /// Container-assigned track id.
    pub id: u32,
    /// Stream kind.
    pub kind: StreamKind,
    /// Codec name as a stable lowercase string (`"h264"`, `"av1"`, `"aac"`, ...), for display and routing.
    pub codec: String,
    /// Container time base for packet timestamps.
    pub time_base: Rational,
    /// BCP-47-ish language tag if the container has one.
    pub language: Option<String>,
    /// Codec configuration record (avcC, av1C, vpcC, AudioSpecificConfig, Opus `dOps`/`OpusHead`,
    /// FLAC `dfLa`/header blocks, Vorbis headers, ...) exactly as stored in the container.
    pub extra_data: Vec<u8>,
    /// Present for video streams.
    pub video: Option<VideoInfo>,
    /// Present for audio streams.
    pub audio: Option<AudioInfo>,
    /// Track duration in microseconds, if the container states or implies one.
    pub duration_us: Option<Timestamp>,
}

/// A cover picture as stored in a file (JPEG or PNG bytes, not decoded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Art {
    /// MIME type, for example `image/jpeg`.
    pub mime: String,
    /// The encoded image.
    pub data: Vec<u8>,
}

/// Descriptive tags read from the container or file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    /// Title.
    pub title: Option<String>,
    /// Artist.
    pub artist: Option<String>,
    /// Album.
    pub album: Option<String>,
    /// Album artist.
    pub album_artist: Option<String>,
    /// Track number within its disc.
    pub track: Option<u32>,
    /// Number of tracks on the disc, if the tag says.
    pub track_total: Option<u32>,
    /// Disc number.
    pub disc: Option<u32>,
    /// Number of discs, if the tag says.
    pub disc_total: Option<u32>,
    /// Year (the first four digits of a date).
    pub year: Option<i32>,
    /// Genre, as text.
    pub genre: Option<String>,
    /// Cover art.
    pub art: Option<Art>,
}

/// A chapter mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    /// Start, microseconds.
    pub start_us: Timestamp,
    /// Title (may be empty).
    pub title: String,
}

/// One compressed access unit from a demuxer.
#[derive(Debug, Clone, PartialEq)]
pub struct Packet {
    /// Track id this packet belongs to.
    pub stream_id: u32,
    /// Presentation time in microseconds.
    pub pts: Timestamp,
    /// Decode time in microseconds (equals `pts` when the container has no reordering).
    pub dts: Timestamp,
    /// Duration in microseconds, 0 if unknown.
    pub duration: Timestamp,
    /// True if decoding can start here.
    pub keyframe: bool,
    /// Microseconds at the end of this packet's decoded audio that must be discarded (Matroska `DiscardPadding`,
    /// used by Opus to cut the encoder's end padding). Zero for everything else.
    pub discard_end_us: Timestamp,
    /// Compressed payload.
    pub data: Vec<u8>,
}

/// Pixel layout of a decoded frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Planar 4:2:0, 8 bits per sample.
    Yuv420p8,
    /// Planar 4:2:0, 10 bits per sample stored in 16-bit little-endian words.
    Yuv420p10,
}

/// YUV to RGB matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMatrix {
    /// ITU-R BT.601.
    Bt601,
    /// ITU-R BT.709.
    Bt709,
    /// ITU-R BT.2020 non-constant luminance.
    Bt2020,
}

/// Sample range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorRange {
    /// Studio swing (16-235).
    Limited,
    /// Full swing (0-255).
    Full,
}

/// A decoded picture.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoFrame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Layout of `planes`.
    pub format: PixelFormat,
    /// Colour matrix.
    pub matrix: ColorMatrix,
    /// Sample range.
    pub range: ColorRange,
    /// Y, U, V planes.
    pub planes: [Vec<u8>; 3],
    /// Row stride in bytes for each plane.
    pub strides: [usize; 3],
    /// Presentation time in microseconds.
    pub pts: Timestamp,
}

/// Requested or negotiated audio output format. Samples are always interleaved `f32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioParams {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channel count (1 or 2 for the v1 output path).
    pub channels: u16,
}

/// Decoded audio, interleaved `f32` in the range -1.0..=1.0.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioBuffer {
    /// Format of `samples`.
    pub params: AudioParams,
    /// Interleaved samples.
    pub samples: Vec<f32>,
    /// Presentation time of the first frame, microseconds.
    pub pts: Timestamp,
}

impl AudioBuffer {
    /// Number of sample frames (samples per channel).
    pub fn frames(&self) -> usize {
        self.samples.len() / self.params.channels.max(1) as usize
    }

    /// Duration in microseconds.
    pub fn duration_us(&self) -> Timestamp {
        (self.frames() as i64 * 1_000_000) / self.params.sample_rate.max(1) as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_duration() {
        let b = AudioBuffer {
            params: AudioParams { sample_rate: 48_000, channels: 2 },
            samples: alloc::vec![0.0; 48_000 * 2],
            pts: 0,
        };
        assert_eq!(b.frames(), 48_000);
        assert_eq!(b.duration_us(), 1_000_000);
    }
}
