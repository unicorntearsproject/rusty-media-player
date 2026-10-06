//! The `#[repr(C)]` structs that cross the boundary, with compile-time size and offset asserts that match the offset tables
//! of the App API pages (draft v0.3). Pointer fields are `u32` offsets into linear memory, as on wasm32; use [`crate::ptr32`]
//! to make one.

macro_rules! layout {
    ($ty:ident = $size:expr, { $($field:ident : $off:expr),* $(,)? }) => {
        const _: () = {
            assert!(core::mem::size_of::<$ty>() == $size);
            $( assert!(core::mem::offset_of!($ty, $field) == $off); )*
        };
    };
}

/// Canvas info (host-to-app, 32 bytes): `canvas_info`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CanvasInfo {
    /// Size of this struct as the app allocated it; the host writes back the size it filled.
    pub struct_size: u32,
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// Device scale (fractional values such as 1.25 are allowed).
    pub scale: f32,
    /// Pixel format (1 = RGBA8).
    pub format: u32,
    /// Display refresh in millihertz (60000 = 60 Hz); 0 = unknown or variable.
    pub refresh_mhz: u32,
    /// Bit 0 fullscreen, bit 1 visible, bit 2 the display is HDR.
    pub flags: u32,
    /// Reserved.
    pub reserved: u32,
}
layout!(CanvasInfo = 32, { struct_size: 0, width: 4, height: 8, scale: 12, format: 16, refresh_mhz: 20, flags: 24, reserved: 28 });

/// Audio-open result (host-to-app, 16 bytes): `audio_open`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AudioOpenInfo {
    /// Size of this struct.
    pub struct_size: u32,
    /// Granted sample rate.
    pub rate: u32,
    /// Granted channel count (1 or 2).
    pub channels: u32,
    /// Ring capacity in frames at the granted rate (at least one second).
    pub capacity_frames: u32,
}
layout!(AudioOpenInfo = 16, { struct_size: 0, rate: 4, channels: 8, capacity_frames: 12 });

/// Audio clock snapshot (host-to-app, 32 bytes): `audio_clock`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AudioClockInfo {
    /// Size of this struct.
    pub struct_size: u32,
    /// Underruns since open or flush.
    pub underruns: u32,
    /// Frames heard since open or flush.
    pub frames_played: u64,
    /// `time_now_us` at which `frames_played` was true.
    pub host_time_us: i64,
    /// Delay from hand-off to audible, microseconds.
    pub latency_us: i64,
}
layout!(AudioClockInfo = 32, { struct_size: 0, underruns: 4, frames_played: 8, host_time_us: 16, latency_us: 24 });

/// Now-playing metadata (app-to-host, 64 bytes): `now_playing_metadata`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NowPlayingMetaRaw {
    /// Size of this struct.
    pub struct_size: u32,
    /// Reserved flags.
    pub flags: u32,
    /// Title pointer (UTF-8; empty = unknown).
    pub title_ptr: u32,
    /// Title length.
    pub title_len: u32,
    /// Artist pointer.
    pub artist_ptr: u32,
    /// Artist length.
    pub artist_len: u32,
    /// Album pointer.
    pub album_ptr: u32,
    /// Album length.
    pub album_len: u32,
    /// Art MIME type pointer (`image/png` or `image/jpeg`).
    pub art_mime_ptr: u32,
    /// Art MIME type length.
    pub art_mime_len: u32,
    /// Art bytes pointer (0/0 = none).
    pub art_ptr: u32,
    /// Art bytes length.
    pub art_len: u32,
    /// Duration in microseconds, -1 if unknown.
    pub duration_us: i64,
    /// The item has a picture.
    pub has_video: u32,
    /// Reserved.
    pub reserved: u32,
}
layout!(NowPlayingMetaRaw = 64, {
    struct_size: 0, flags: 4, title_ptr: 8, title_len: 12, artist_ptr: 16, artist_len: 20, album_ptr: 24, album_len: 28,
    art_mime_ptr: 32, art_mime_len: 36, art_ptr: 40, art_len: 44, duration_us: 48, has_video: 56, reserved: 60
});

/// Now-playing playback state (app-to-host, 40 bytes): `now_playing_playback`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NowPlayingPlaybackRaw {
    /// Size of this struct.
    pub struct_size: u32,
    /// 0 stopped, 1 playing, 2 paused.
    pub state: u32,
    /// Playback rate.
    pub rate: f32,
    /// Reserved.
    pub reserved0: u32,
    /// Position in microseconds when `host_time_us` was true.
    pub position_us: i64,
    /// `time_now_us` when `position_us` was true.
    pub host_time_us: i64,
    /// Bit 0 can_next, bit 1 can_prev, bit 2 can_seek.
    pub flags: u32,
    /// Reserved.
    pub reserved1: u32,
}
layout!(NowPlayingPlaybackRaw = 40, {
    struct_size: 0, state: 4, rate: 8, reserved0: 12, position_us: 16, host_time_us: 24, flags: 32, reserved1: 36
});

/// Visualizer summary (app-to-host, 176 bytes): `viz_summary` and `viz_summary_n`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VizSummaryRaw {
    /// Size of this struct.
    pub struct_size: u32,
    /// 1 when an onset begins in this hop.
    pub onset: u32,
    /// Stream time of the hop start, microseconds.
    pub pts_us: i64,
    /// RMS level, 0..=1.
    pub level: f32,
    /// Peak, 0..=1.
    pub peak: f32,
    /// 32 log-spaced bands, 0..=1 on a -70..0 dB scale.
    pub bands: [f32; 32],
    /// Mean of bands 0..8.
    pub bass: f32,
    /// Mean of bands 8..24.
    pub mid: f32,
    /// Mean of bands 24..32.
    pub treble: f32,
    /// Onset strength.
    pub onset_strength: f32,
    /// Tempo, 0.0 while unknown.
    pub tempo_bpm: f32,
    /// Reserved.
    pub reserved: u32,
}
layout!(VizSummaryRaw = 176, {
    struct_size: 0, onset: 4, pts_us: 8, level: 16, peak: 20, bands: 24, bass: 152, mid: 156, treble: 160,
    onset_strength: 164, tempo_bpm: 168, reserved: 172
});

impl Default for VizSummaryRaw {
    fn default() -> Self {
        Self {
            struct_size: core::mem::size_of::<Self>() as u32,
            onset: 0,
            pts_us: 0,
            level: 0.0,
            peak: 0.0,
            bands: [0.0; 32],
            bass: 0.0,
            mid: 0.0,
            treble: 0.0,
            onset_strength: 0.0,
            tempo_bpm: 0.0,
            reserved: 0,
        }
    }
}

/// A video frame (app-to-host, 104 bytes): `video_present`. It carries the same information as `rvp_core::VideoFrame`,
/// plus a destination rectangle.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VideoFrameRaw {
    /// Size of this struct.
    pub struct_size: u32,
    /// Visible width.
    pub width: u32,
    /// Visible height.
    pub height: u32,
    /// 0 YUV 4:2:0 8-bit, 1 YUV 4:2:0 10-bit.
    pub format: u32,
    /// 0 BT.601, 1 BT.709, 2 BT.2020.
    pub matrix: u32,
    /// 0 limited, 1 full.
    pub range: u32,
    /// Bits 0-3 transfer, bits 4-7 primaries, bit 8 nearest-neighbour scaling.
    pub flags: u32,
    /// Reserved.
    pub reserved: u32,
    /// Plane pointers (Y, U, V, 0).
    pub planes: [u32; 4],
    /// Plane lengths in bytes.
    pub plane_lens: [u32; 4],
    /// Strides in **bytes**.
    pub strides: [u32; 4],
    /// Presentation time, microseconds.
    pub pts_us: i64,
    /// Destination in canvas pixels (x, y, w, h); all zero hides the layer.
    pub dest: [i32; 4],
}
layout!(VideoFrameRaw = 104, {
    struct_size: 0, width: 4, height: 8, format: 12, matrix: 16, range: 20, flags: 24, reserved: 28,
    planes: 32, plane_lens: 48, strides: 64, pts_us: 80, dest: 88
});

/// The 64-byte event record `events_wait` writes.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Event {
    /// Event kind (see [`crate::ev`]).
    pub kind: u16,
    /// Per-kind flags.
    pub flags: u16,
    /// Reserved.
    pub reserved: u32,
    /// Time of the event, in the `time_now_us` clock.
    pub time_us: i64,
    /// The payload (starts at record offset 16).
    pub payload: [u8; 48],
}
layout!(Event = 64, { kind: 0, flags: 2, reserved: 4, time_us: 8, payload: 16 });

impl Default for Event {
    fn default() -> Self {
        Self { kind: 0, flags: 0, reserved: 0, time_us: 0, payload: [0; 48] }
    }
}

impl core::fmt::Debug for Event {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Event {{ kind: {}, flags: {}, time_us: {} }}", self.kind, self.flags, self.time_us)
    }
}

impl Event {
    /// An event with the given kind, flags and time and an empty payload.
    pub fn new(kind: u16, flags: u16, time_us: i64) -> Self {
        Self { kind, flags, reserved: 0, time_us, payload: [0; 48] }
    }

    /// The bytes of the record, little-endian, as the host writes them.
    pub fn to_bytes(&self) -> [u8; 64] {
        let mut b = [0u8; 64];
        b[0..2].copy_from_slice(&self.kind.to_le_bytes());
        b[2..4].copy_from_slice(&self.flags.to_le_bytes());
        b[4..8].copy_from_slice(&self.reserved.to_le_bytes());
        b[8..16].copy_from_slice(&self.time_us.to_le_bytes());
        b[16..].copy_from_slice(&self.payload);
        b
    }

    /// An event from the 64 bytes the host wrote.
    pub fn from_bytes(b: &[u8; 64]) -> Self {
        let mut payload = [0u8; 48];
        payload.copy_from_slice(&b[16..]);
        Self {
            kind: u16::from_le_bytes([b[0], b[1]]),
            flags: u16::from_le_bytes([b[2], b[3]]),
            reserved: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            time_us: i64::from_le_bytes([b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]]),
            payload,
        }
    }

    fn at<const N: usize>(&self, record_offset: usize) -> [u8; N] {
        let mut out = [0u8; N];
        if let Some(src) = record_offset.checked_sub(16).and_then(|o| self.payload.get(o..o + N)) {
            out.copy_from_slice(src);
        }
        out
    }

    /// A `u32` at `record_offset` (an offset from the start of the record, 16 or more, as the event tables give them).
    pub fn u32_at(&self, record_offset: usize) -> u32 {
        u32::from_le_bytes(self.at(record_offset))
    }

    /// An `i32` at `record_offset`.
    pub fn i32_at(&self, record_offset: usize) -> i32 {
        i32::from_le_bytes(self.at(record_offset))
    }

    /// An `f32` at `record_offset`.
    pub fn f32_at(&self, record_offset: usize) -> f32 {
        f32::from_le_bytes(self.at(record_offset))
    }

    /// An `i64` at `record_offset`.
    pub fn i64_at(&self, record_offset: usize) -> i64 {
        i64::from_le_bytes(self.at(record_offset))
    }

    /// A `u64` at `record_offset`.
    pub fn u64_at(&self, record_offset: usize) -> u64 {
        u64::from_le_bytes(self.at(record_offset))
    }

    /// Write a `u32` at `record_offset` (for hosts and mocks).
    pub fn put_u32(&mut self, record_offset: usize, v: u32) {
        self.put(record_offset, &v.to_le_bytes());
    }

    /// Write an `i32` at `record_offset`.
    pub fn put_i32(&mut self, record_offset: usize, v: i32) {
        self.put(record_offset, &v.to_le_bytes());
    }

    /// Write an `f32` at `record_offset`.
    pub fn put_f32(&mut self, record_offset: usize, v: f32) {
        self.put(record_offset, &v.to_le_bytes());
    }

    /// Write an `i64` at `record_offset`.
    pub fn put_i64(&mut self, record_offset: usize, v: i64) {
        self.put(record_offset, &v.to_le_bytes());
    }

    /// Write a `u64` at `record_offset`.
    pub fn put_u64(&mut self, record_offset: usize, v: u64) {
        self.put(record_offset, &v.to_le_bytes());
    }

    fn put(&mut self, record_offset: usize, bytes: &[u8]) {
        if let Some(dst) =
            record_offset.checked_sub(16).and_then(|o| self.payload.get_mut(o..o + bytes.len()))
        {
            dst.copy_from_slice(bytes);
        }
    }
}

/// Size of one event record.
pub const EVENT_SIZE: usize = 64;
