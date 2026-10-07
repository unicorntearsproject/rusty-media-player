//! A playback session: one opened media item, its demux and decode tasks, and the output side that feeds the
//! host's audio sink and keeps the master clock.
//!
//! Threading model (docs/PLAN.md section 5): everything runs on the caller's thread. `Session::tick` polls the
//! demux and decode tasks (cooperatively, within a time budget) and then does the synchronous output work:
//! feed the audio sink, update the clock. Tasks never touch the host; they communicate through [`Shared`].
use crate::audio::{AudioOut, FadeStatus, LevelConfig, TraceEntry};
use crate::exec::Executor;
use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use rvp_core::task::yield_now;
use rvp_core::{
    AudioBuffer, AudioParams, AudioSettings, ClockSource, CodecFactory, Error, LevelMode, LoudnessTags,
    MasterClock, Metadata, Packet, StreamInfo, StreamKind, Timestamp, VideoFrame,
};
use rvp_demux::{Demuxer, open};
use rvp_host::{AudioSink, Host, Source, VideoSink};
use rvp_subs::{AssScript, Cue, CueList, pgs::PgsDecoder, pgs::Update as PgsUpdate};

/// First id given to subtitle tracks that do not come from the container (sidecar files).
pub const EXTERNAL_TRACK_BASE: u32 = 0x1000_0000;
/// Largest sidecar subtitle file read, bytes.
const MAX_SUB_FILE: usize = 8 << 20;
/// A cue without a known duration stays up this long, microseconds.
const DEFAULT_CUE_US: i64 = 3_000_000;
/// A PGS picture without a known duration stays up until the next display set, but never longer than this.
const PGS_MAX_US: i64 = 60_000_000;
/// After a seek, subtitle packets this far before the target are read again, so cues that began earlier but are
/// still on screen show (the demuxer lands on a video keyframe and reads on from there).
const SUB_LOOKBACK_US: i64 = 20_000_000;

/// Packets buffered between the demuxer and a decoder.
const MAX_PACKETS: usize = 128;
/// Picture packets the demuxer may have queued while the sound is short of audio (a slow machine falls behind on pictures, and the sound's packets
/// are further along the file: the demuxer must keep reading, or the sound starves behind a backlog of pictures).
const MAX_VIDEO_PACKETS_AUDIO_FIRST: usize = 3 * MAX_PACKETS;
/// When this many picture packets are waiting and the oldest is later than `VIDEO_LATE_US` behind the playback clock, the video task jumps to the
/// newest keyframe among them: it cannot keep up (a slow machine), and the demuxer behind the backlog would starve the sound.
const VIDEO_BACKLOG_PACKETS: usize = 96;
/// How far behind the clock the oldest waiting picture packet must be for that jump.
const VIDEO_LATE_US: i64 = 1_500_000;
/// Compressed bytes the demuxer may have queued for one stream before it waits (a hostile file can make packets huge).
const MAX_QUEUED_BYTES: usize = 96 << 20;
/// How long a file that ends in the middle of a packet is waited for (it may be still growing), and how often the
/// demuxer looks again.
const GROW_GRACE_US: i64 = 1_500_000;
const GROW_RETRY_US: i64 = 250_000;
/// Decoded video frames kept ready for presentation.
const MAX_VIDEO_FRAMES: usize = 6;
/// Decoded audio kept ahead of the output stage, microseconds.
const AUDIO_AHEAD_US: i64 = 800_000;
/// Packets the audio decoder takes in one turn of the scheduler: when the host's frames come late (a slow redraw in a browser) one turn must
/// refill what the late frame let drain, not a single packet's worth.
const AUDIO_PACKETS_PER_TURN: usize = 16;
/// Audio we want queued (pending + sink) before starting playback, microseconds.
const START_BUFFER_US: i64 = 100_000;
/// Audio-only seeks start this much earlier and discard up to the target, so codecs with overlap state
/// (Opus, Vorbis, AAC, MP3) are warm when the target is reached.
const PREROLL_US: i64 = 100_000;
/// Preferred output format; the host may answer with something else.
const WANT_AUDIO: AudioParams = AudioParams { sample_rate: 48_000, channels: 2 };
/// How often `tick` wants to run while playing.
const TICK_US: i64 = 10_000;
/// Wall-clock budget for the task polling part of a tick.
const BUDGET_US: i64 = 8_000;
/// Sound comes first. Once a turn has spent its budget (a big picture takes much longer than that to decode, most of all in a browser's single
/// thread), it goes on with cheap rounds only (demuxing and audio decoding, no more pictures) for at most this long while the audio queued ahead
/// is below `AUDIO_FEED_US`, so a slow turn cannot leave the output without sound.
const AUDIO_ONLY_US: i64 = 25_000;
/// How much audio must be waiting (decoded, queued for the sink and in the sink) before a turn that is out of budget stops feeding it.
const AUDIO_FEED_US: i64 = 900_000;
/// Rounds of the task loop in one turn, at most.
const MAX_ROUNDS: usize = 256;
/// The caller should queue the next item when this little of the current one is left, microseconds (plus the length of the
/// crossfade, when there is one, so the next item is open and decoding before the fade has to start).
const NEXT_LEAD_US: i64 = 12_000_000;
/// A crossfade is never made shorter than this (a shorter one is a click, not a fade): such a join is gapless instead.
const MIN_FADE_US: i64 = 250_000;
/// A crossfade takes at most this share of the shorter of the two items, so a short track is not all fade.
const FADE_MAX_SHARE: i64 = 3;

/// Playback state of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Opening the file.
    Opening,
    /// Open; not playing.
    Paused,
    /// Play requested, waiting for enough decoded data (initial buffering or after a seek).
    Buffering,
    /// Playing.
    Playing,
    /// Everything played out.
    Ended,
    /// Opening failed; see [`Session::error`].
    Failed,
}

/// A subtitle track: from the container or from a sidecar file.
#[derive(Debug, Clone)]
pub struct SubtitleTrack {
    /// Container track id, or an id from [`EXTERNAL_TRACK_BASE`] up for sidecar files.
    pub id: u32,
    /// Language tag if known.
    pub language: Option<String>,
    /// Display name: the language, the file name, or "Subtitles n".
    pub label: String,
    /// True for sidecar files.
    pub external: bool,
}

/// What a subtitle track's packets hold.
enum SubKind {
    /// Plain text: Matroska SRT/WebVTT, MP4 `wvtt`.
    Text,
    /// MP4 `tx3g` samples.
    MovText,
    /// Matroska ASS/SSA blocks, with the script header from the track's codec private data.
    Ass(Box<AssScript>),
    /// PGS display sets.
    Pgs(Box<PgsDecoder>),
    /// A sidecar file, already parsed.
    File,
}

struct SubSlot {
    track: SubtitleTrack,
    cues: CueList,
    kind: SubKind,
}

impl SubSlot {
    /// Turn one container packet into a cue (or, for PGS, into a change of what is on screen).
    fn ingest(&mut self, p: &Packet) {
        let (start, dur) = (p.pts, p.duration);
        let end = start + if dur > 0 { dur } else { DEFAULT_CUE_US };
        match &mut self.kind {
            SubKind::Ass(script) => {
                let dur = if dur > 0 { dur } else { DEFAULT_CUE_US };
                if let Some(cue) = script.cue_from_block(&p.data, start, dur) {
                    self.cues.insert(cue);
                }
            }
            SubKind::Pgs(dec) => match dec.decode(&p.data) {
                PgsUpdate::None => {}
                PgsUpdate::Clear => self.cues.clip_images(start),
                PgsUpdate::Show(img) => {
                    self.cues.clip_images(start);
                    let mut cue = Cue::new(
                        start,
                        if dur > 0 { start + dur } else { start + PGS_MAX_US },
                        String::new(),
                    );
                    cue.image = Some(img);
                    self.cues.insert(cue);
                }
            },
            kind => {
                let text = match p.data.get(4..8) {
                    Some(b"vttc") | Some(b"vtte") => rvp_subs::decode_wvtt_sample(&p.data),
                    _ if matches!(kind, SubKind::MovText) => rvp_subs::decode_mov_text(&p.data),
                    _ => rvp_subs::decode_mkv_text(&p.data),
                };
                if let Some(text) = text {
                    self.cues.insert(Cue::new(start, end, text));
                }
            }
        }
    }
}

/// The plain text of cues on screen together, lines of different cues one under the other.
fn join_text(cues: &[Cue]) -> Option<String> {
    let mut out = String::new();
    for c in cues.iter().filter(|c| !c.text.is_empty()) {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&c.text);
    }
    (!out.is_empty()).then_some(out)
}

/// English name of a language tag (ISO 639-1/2 for the common ones), or the tag itself.
pub fn language_name(tag: &str) -> String {
    let t = tag.to_ascii_lowercase();
    let base = t.split(['-', '_']).next().unwrap_or("");
    let name = match base {
        "en" | "eng" => "English",
        "es" | "spa" => "Spanish",
        "fr" | "fre" | "fra" => "French",
        "de" | "ger" | "deu" => "German",
        "it" | "ita" => "Italian",
        "pt" | "por" => "Portuguese",
        "nl" | "dut" | "nld" => "Dutch",
        "sv" | "swe" => "Swedish",
        "pl" | "pol" => "Polish",
        "ru" | "rus" => "Russian",
        "ja" | "jpn" => "Japanese",
        "ko" | "kor" => "Korean",
        "zh" | "chi" | "zho" => "Chinese",
        "ar" | "ara" => "Arabic",
        "hi" | "hin" => "Hindi",
        "tr" | "tur" => "Turkish",
        _ => return String::from(tag),
    };
    String::from(name)
}

/// Something that happened that a UI or host may want to react to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// The subtitle on screen changed; `text` is `None` when it cleared. `at_us` is the stream position at which
    /// the change was noticed.
    Subtitle {
        /// Stream position when noticed, microseconds.
        at_us: Timestamp,
        /// The new text, if any.
        text: Option<String>,
    },
    /// Another item began to be heard (gapless chain or after the previous one ended). `tag` is the value given
    /// to [`Session::queue_next`].
    ItemStarted {
        /// The caller's tag for the item.
        tag: u32,
    },
    /// The queued next item could not be opened and was dropped.
    ItemFailed {
        /// The caller's tag for the item.
        tag: u32,
        /// What went wrong.
        error: Error,
    },
}

/// The next item of a chain, opened and decoding ahead while the current one plays.
struct NextItem {
    exec: Executor,
    sh: Sh,
    tag: u32,
    /// What the caller knows about the item's loudness (the library's measurement).
    hint: Option<LoudnessTags>,
}

/// A crossfade in progress: both items' shared state (the first one's tasks are kept running after the switch of the
/// current item in the middle of the fade, for the half of the fade that is still to come).
struct FadeState {
    a: Sh,
    b: Sh,
    old_exec: Option<Executor>,
}

/// True when two items are consecutive tracks of one album that is marked as gapless (the iTunes "gapless album" flag): they
/// were made to run into each other and a crossfade would cut across the music. Tracks without numbers count as consecutive.
fn same_gapless_album(a: &Metadata, b: &Metadata) -> bool {
    let album = |m: &Metadata| m.album.as_deref().map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty());
    if !(a.gapless_album && b.gapless_album) || album(a).is_none() || album(a) != album(b) {
        return false;
    }
    match (a.track, b.track) {
        (Some(ta), Some(tb)) => {
            let (da, db) = (a.disc.unwrap_or(1), b.disc.unwrap_or(1));
            (da == db && tb == ta + 1) || (db == da + 1 && tb == 1)
        }
        _ => true,
    }
}

/// The loudness of an item for the level mode: from its own tags first, then from what the caller knows (the library's
/// measurement). The album's loudness, in album mode, and the track's when there is no album figure.
fn known_loudness(mode: LevelMode, tags: &LoudnessTags, hint: Option<&LoudnessTags>) -> Option<f32> {
    let track = tags.track_lufs.or(hint.and_then(|h| h.track_lufs));
    match mode {
        LevelMode::Track => track,
        LevelMode::Album => tags.album_lufs.or(hint.and_then(|h| h.album_lufs)).or(track),
    }
}

/// End of the item's playable audio, microseconds: the audio stream's length, else the container's.
fn audio_end(s: &Shared) -> Option<Timestamp> {
    s.sel_audio.as_ref().and_then(|a| a.duration_us).or(s.duration_us)
}

/// Take the next decoded audio buffer of an item, cut at the end of the track's playable length (an MP4 edit list cuts the
/// encoder's padding).
fn pop_audio(sh: &Sh) -> Option<AudioBuffer> {
    loop {
        let mut s = sh.borrow_mut();
        let b = s.audio_dec.pop_front()?;
        s.audio_dec_us -= buffer_us(&b);
        let limit = s.sel_audio.as_ref().and_then(|a| a.duration_us);
        drop(s);
        match limit {
            Some(l) => {
                if let Some(b) = limit_end(b, l) {
                    return Some(b);
                }
            }
            None => return Some(b),
        }
    }
}

#[derive(Default)]
struct Shared {
    subs: Vec<SubSlot>,
    sel_sub: Option<u32>,
    next_external: u32,
    meta: rvp_core::Metadata,
    chapters: Vec<rvp_core::Chapter>,
    /// Bumped when the selected audio stream changes; the audio task rebuilds its decoder.
    audio_epoch: u32,
    opened: bool,
    streams: Vec<StreamInfo>,
    duration_us: Option<Timestamp>,
    error: Option<Error>,
    sel_audio: Option<StreamInfo>,
    sel_video: Option<StreamInfo>,
    warnings: Vec<String>,
    video_in: VecDeque<Packet>,
    video_dec: VecDeque<VideoFrame>,
    video_done: bool,
    /// Frames before this stream time are skipped after a seek (all but the last one at or before it).
    video_floor: Timestamp,
    audio_in: VecDeque<Packet>,
    audio_dec: VecDeque<AudioBuffer>,
    audio_dec_us: i64,
    demux_done: bool,
    audio_done: bool,
    seek: Option<Timestamp>,
    seeking: bool,
    landed: Option<Timestamp>,
    generation: u32,
    /// Bumped whenever a task moves data; lets `tick` stop polling when nothing changes.
    progress: u64,
    /// Host time of the current tick (the tasks have no clock of their own).
    now: Timestamp,
    /// Whether the video task may decode this round: false once the turn's budget is spent (the rest of the turn is for the audio).
    video_ok: bool,
    /// Whether the sound is short of audio (set by the turn): the demuxer then reads on even though the pictures are behind.
    audio_low: bool,
    /// Where the playback clock is, stream time (set by the turn): how late a waiting picture packet is.
    clock_us: Timestamp,
    /// Picture packets dropped by jumping to a keyframe because the decoder fell behind.
    video_skipped: u64,
    /// When the demuxer first hit the end of a file that is cut off (still being written?), and when to look again.
    cut_since: Option<Timestamp>,
    cut_retry_at: Timestamp,
}

type Sh = Rc<RefCell<Shared>>;

fn buffer_us(b: &AudioBuffer) -> i64 {
    b.duration_us()
}

/// Create the shared state and the demux, audio and video tasks for one item.
fn spawn_item<S: Source + 'static>(source: S, codecs: &Rc<dyn CodecFactory>) -> (Executor, Sh) {
    let sh: Sh = Rc::default();
    let mut exec = Executor::new();
    exec.spawn(demux_task(source, sh.clone()));
    exec.spawn(audio_task(sh.clone(), codecs.clone()));
    exec.spawn(video_task(sh.clone(), codecs.clone()));
    (exec, sh)
}

async fn demux_task<S: Source + 'static>(source: S, sh: Sh) {
    let mut d = match open(source).await {
        Ok(d) => d,
        Err(e) => {
            let mut s = sh.borrow_mut();
            s.error = Some(e);
            s.demux_done = true;
            return;
        }
    };
    {
        let mut s = sh.borrow_mut();
        s.streams = d.streams().to_vec();
        s.duration_us = d.duration_us();
        s.meta = d.metadata().clone();
        s.chapters = d.chapters().to_vec();
        s.sel_audio = s.streams.iter().find(|i| i.kind == StreamKind::Audio).cloned();
        s.sel_video = s.streams.iter().find(|i| i.kind == StreamKind::Video).cloned();
        let tracks: Vec<StreamInfo> =
            s.streams.iter().filter(|i| i.kind == StreamKind::Subtitle).cloned().collect();
        for (n, st) in tracks.iter().enumerate() {
            if !matches!(st.codec.as_str(), "subrip" | "webvtt" | "mov_text" | "ass" | "hdmv_pgs") {
                s.warnings.push(alloc::format!("subtitle track {} ({}) is not supported", st.id, st.codec));
                continue;
            }
            let label = match &st.language {
                Some(l) if !l.is_empty() && l != "und" => language_name(l),
                _ => alloc::format!("Subtitles {}", n + 1),
            };
            s.subs.push(SubSlot {
                track: SubtitleTrack { id: st.id, language: st.language.clone(), label, external: false },
                cues: CueList::new(),
                kind: match st.codec.as_str() {
                    "mov_text" => SubKind::MovText,
                    "ass" => SubKind::Ass(Box::new(AssScript::parse_header(&String::from_utf8_lossy(
                        &st.extra_data,
                    )))),
                    "hdmv_pgs" => SubKind::Pgs(Box::default()),
                    _ => SubKind::Text,
                },
            });
        }
        s.opened = true;
        s.progress += 1;
    }
    loop {
        let req = sh.borrow_mut().seek.take();
        if let Some(target) = req {
            let res = d.seek(target).await;
            // Cues that began before the landing point but are still showing at the target: read the subtitle
            // packets of the stretch before it again (an equal cue is not added twice).
            if let Ok(landed) = res {
                let ids: Vec<u32> = sh
                    .borrow()
                    .subs
                    .iter()
                    .filter(|t| !t.track.external && !matches!(t.kind, SubKind::File))
                    .map(|t| t.track.id)
                    .collect();
                if !ids.is_empty() {
                    let from = target.min(landed) - SUB_LOOKBACK_US;
                    if let Ok(pkts) = d.side_packets(&ids, from, landed.max(target)).await {
                        let mut s = sh.borrow_mut();
                        for p in &pkts {
                            if let Some(slot) = s.subs.iter_mut().find(|t| t.track.id == p.stream_id) {
                                slot.ingest(p);
                            }
                        }
                    }
                }
            }
            let mut s = sh.borrow_mut();
            s.audio_in.clear();
            s.audio_dec.clear();
            s.audio_dec_us = 0;
            s.video_in.clear();
            s.video_dec.clear();
            s.demux_done = false;
            s.audio_done = false;
            s.video_done = false;
            match res {
                Ok(l) => s.landed = Some(l),
                Err(e) => s.error = Some(e),
            }
            s.seeking = false;
            s.progress += 1;
            continue;
        }
        let blocked = {
            let s = sh.borrow();
            s.demux_done
                || s.audio_in.len() >= MAX_PACKETS
                || s.video_in.len() >= if s.audio_low { MAX_VIDEO_PACKETS_AUDIO_FIRST } else { MAX_PACKETS }
                || s.audio_in.iter().map(|p| p.data.len()).sum::<usize>() >= MAX_QUEUED_BYTES
                || s.video_in.iter().map(|p| p.data.len()).sum::<usize>() >= MAX_QUEUED_BYTES
                || s.now < s.cut_retry_at
        };
        if blocked {
            yield_now().await;
            continue;
        }
        let res = d.next_packet().await;
        {
            let mut s = sh.borrow_mut();
            // A seek that arrived while this read was in flight makes the packet stale: drop it.
            if !(s.seek.is_some() || s.seeking) {
                match res {
                    Ok(Some(p)) => {
                        s.cut_since = None;
                        if s.sel_audio.as_ref().is_some_and(|a| a.id == p.stream_id) {
                            s.audio_in.push_back(p);
                        } else if s.sel_video.as_ref().is_some_and(|v| v.id == p.stream_id) {
                            s.video_in.push_back(p);
                        } else if let Some(slot) = s.subs.iter_mut().find(|t| t.track.id == p.stream_id) {
                            slot.ingest(&p);
                        }
                        s.progress += 1;
                    }
                    Ok(None) => s.demux_done = true,
                    Err(Error::Truncated) => {
                        // Cut off in the middle of a packet: wait a little, as the file may still be growing (the
                        // demuxers look at the length again on every call), then play what there is.
                        let now = s.now;
                        let since = *s.cut_since.get_or_insert(now);
                        if s.now - since < GROW_GRACE_US {
                            s.cut_retry_at = s.now + GROW_RETRY_US;
                        } else {
                            s.warnings.push(String::from("The file ends early: playing what is there."));
                            s.error = Some(Error::Truncated);
                            s.demux_done = true;
                        }
                    }
                    Err(e) => {
                        s.error = Some(e);
                        s.demux_done = true;
                    }
                }
            }
        }
        yield_now().await;
    }
}

/// Remove `us` microseconds from the end of the audio in `bufs` (a packet's decoded output).
fn trim_end(bufs: &mut Vec<AudioBuffer>, us: i64) {
    let Some(rate) = bufs.last().map(|b| b.params.sample_rate as i64) else { return };
    let mut drop_frames = ((us as i128 * rate as i128 + 500_000) / 1_000_000) as usize;
    while drop_frames > 0 {
        let Some(last) = bufs.last_mut() else { break };
        let ch = last.params.channels.max(1) as usize;
        let frames = last.samples.len() / ch;
        if frames <= drop_frames {
            drop_frames -= frames;
            bufs.pop();
        } else {
            last.samples.truncate((frames - drop_frames) * ch);
            break;
        }
    }
}

/// Cut `b` so it ends at stream time `limit_us` (the audio track's playable length); `None` if nothing is left.
fn limit_end(mut b: AudioBuffer, limit_us: Timestamp) -> Option<AudioBuffer> {
    if b.pts >= limit_us {
        return None;
    }
    let ch = b.params.channels.max(1) as usize;
    let frames = b.samples.len() / ch;
    let keep = ((limit_us - b.pts) as i128 * b.params.sample_rate as i128 / 1_000_000) as usize;
    if keep < frames {
        b.samples.truncate(keep * ch);
    }
    (!b.samples.is_empty()).then_some(b)
}

async fn audio_task(sh: Sh, codecs: Rc<dyn CodecFactory>) {
    while !sh.borrow().opened {
        yield_now().await;
    }
    let mut audio_epoch = sh.borrow().audio_epoch;
    let mut dec = {
        let info = sh.borrow().sel_audio.clone();
        match info.as_ref().map(|i| codecs.audio(i)) {
            Some(Ok(d)) => d,
            other => {
                let mut s = sh.borrow_mut();
                if let Some(Err(e)) = other {
                    s.warnings.push(alloc::format!("audio disabled: {e}"));
                }
                s.sel_audio = None;
                s.audio_done = true;
                return;
            }
        }
    };
    let mut epoch = sh.borrow().generation;
    loop {
        if sh.borrow().audio_epoch != audio_epoch {
            // Another audio track was selected: build its decoder.
            audio_epoch = sh.borrow().audio_epoch;
            let info = sh.borrow().sel_audio.clone();
            match info.as_ref().map(|i| codecs.audio(i)) {
                Some(Ok(d)) => dec = d,
                other => {
                    let mut s = sh.borrow_mut();
                    if let Some(Err(e)) = other {
                        s.warnings.push(alloc::format!("audio disabled: {e}"));
                    }
                    s.sel_audio = None;
                    s.audio_done = true;
                    return;
                }
            }
        }
        for _ in 0..AUDIO_PACKETS_PER_TURN {
            let packet = {
                let mut s = sh.borrow_mut();
                if s.generation != epoch {
                    epoch = s.generation;
                    dec.flush();
                }
                if s.audio_dec_us >= AUDIO_AHEAD_US {
                    None
                } else {
                    let p = s.audio_in.pop_front();
                    if p.is_none() && s.demux_done && !s.seeking {
                        s.audio_done = true;
                    }
                    p
                }
            };
            let Some(p) = packet else { break };
            // A packet that fails to decode is dropped; playback continues with the next one.
            if dec.send_packet(&p).is_ok() {
                let mut bufs: Vec<AudioBuffer> = Vec::new();
                while let Ok(Some(b)) = dec.receive_buffer() {
                    bufs.push(b);
                }
                if p.discard_end_us > 0 {
                    trim_end(&mut bufs, p.discard_end_us);
                }
                let mut s = sh.borrow_mut();
                for b in bufs {
                    s.audio_dec_us += buffer_us(&b);
                    s.audio_dec.push_back(b);
                }
            }
            sh.borrow_mut().progress += 1;
        }
        yield_now().await;
    }
}

async fn video_task(sh: Sh, codecs: Rc<dyn CodecFactory>) {
    while !sh.borrow().opened {
        yield_now().await;
    }
    let info = sh.borrow().sel_video.clone();
    let mut dec = match info.as_ref().map(|i| codecs.video(i)) {
        Some(Ok(d)) => d,
        other => {
            let mut s = sh.borrow_mut();
            if let Some(Err(e)) = other {
                s.warnings.push(alloc::format!("video disabled: {e}"));
            }
            s.sel_video = None;
            s.video_done = true;
            return;
        }
    };
    let mut epoch = sh.borrow().generation;
    let mut drained = false;
    loop {
        let (packet, drain_now) = {
            let mut s = sh.borrow_mut();
            if s.generation != epoch {
                epoch = s.generation;
                dec.flush();
                drained = false;
            }
            // After a seek keep only the last frame at or before the target (the one to show) and later ones.
            while s.video_dec.len() >= 2 && s.video_dec[1].pts <= s.video_floor {
                s.video_dec.pop_front();
            }
            // Far behind (a slow machine): jump to the newest keyframe of the backlog, skipping the pictures between, and start the decoder
            // clean there. Better a gap in the picture than a decoder that never catches up and a sound that starves behind it.
            let late = s.video_in.front().is_some_and(|p| p.pts + VIDEO_LATE_US < s.clock_us);
            if s.video_in.len() >= VIDEO_BACKLOG_PACKETS && late {
                if let Some(k) = s.video_in.iter().rposition(|p| p.keyframe).filter(|&k| k > 0) {
                    s.video_in.drain(..k);
                    s.video_skipped += k as u64;
                    s.video_dec.clear();
                    dec.flush();
                    drained = false;
                }
            }
            // A decoder on its own thread has packets in flight whose frames are still to come.
            if s.video_dec.len() + dec.pending() >= MAX_VIDEO_FRAMES || !s.video_ok {
                (None, false)
            } else {
                let p = s.video_in.pop_front();
                let drain_now = p.is_none() && s.demux_done && !s.seeking && !drained;
                (p, drain_now)
            }
        };
        if let Some(p) = packet {
            let ok = dec.send_packet(&p).is_ok();
            let mut s = sh.borrow_mut();
            if ok {
                while let Ok(Some(f)) = dec.receive_frame() {
                    s.video_dec.push_back(f);
                }
            } else {
                s.warnings.push(alloc::format!("video packet at {} us failed to decode", p.pts));
            }
            s.progress += 1;
        } else if drain_now {
            let _ = dec.drain();
            let mut s = sh.borrow_mut();
            while let Ok(Some(f)) = dec.receive_frame() {
                s.video_dec.push_back(f);
            }
            drained = true;
            s.progress += 1;
        }
        // Read `pending` before collecting: a threaded decoder publishes frames before it lowers the count, so
        // a zero here means every frame is already visible below.
        let pending = dec.pending();
        {
            let mut s = sh.borrow_mut();
            while let Ok(Some(f)) = dec.receive_frame() {
                s.video_dec.push_back(f);
            }
            if drained && pending == 0 && s.video_in.is_empty() && s.demux_done && !s.seeking {
                s.video_done = true;
            }
        }
        yield_now().await;
    }
}

/// Counters describing how video presentation is going.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VideoStats {
    /// Frames handed to the video sink.
    pub presented: u64,
    /// Frames skipped because a later frame was already due (late).
    pub dropped: u64,
    /// Largest `|clock - frame pts|` at the moment a frame was presented, microseconds.
    pub max_drift_us: i64,
    /// Largest time an old frame stayed on screen while playing (clock minus the last presented pts).
    pub max_staleness_us: i64,
}

/// One recorded presentation (see [`Session::enable_video_trace`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoTraceEntry {
    /// Clock position when presented.
    pub clock_us: Timestamp,
    /// Frame pts.
    pub pts: Timestamp,
}

/// One opened media item and its playback machinery.
pub struct Session {
    exec: Executor,
    sh: Sh,
    clock: MasterClock,
    audio: Option<AudioOut>,
    audio_opened: bool,
    /// What the audio device was last told about pausing.
    sink_paused: bool,
    want_play: bool,
    running: bool,
    needs_sink_flush: bool,
    volume: f32,
    muted: bool,
    rate: f64,
    ended: bool,
    trace: bool,
    seek_target: Option<Timestamp>,
    stats: VideoStats,
    video_trace: Option<Vec<VideoTraceEntry>>,
    shown_preview: bool,
    last_presented: Option<Timestamp>,
    events: VecDeque<SessionEvent>,
    shown_subtitle: Option<String>,
    shown_cues: Vec<Cue>,
    /// Frame steps requested and not yet shown: +1 forward, -1 back (consumed one at a time while paused).
    steps: VecDeque<i8>,
    /// A frame step moved the position, so resuming must re-seek to restart audio there.
    stepped: bool,
    /// A-B loop: when playback reaches B it jumps back to A.
    ab_loop: Option<(Timestamp, Timestamp)>,
    codecs: Rc<dyn CodecFactory>,
    /// The caller's tag of the item being played.
    tag: u32,
    /// Item number stamped on audio segments (increments at every item change).
    item_no: u32,
    viz: Option<rvp_viz::Analyzer>,
    viz_reset: bool,
    /// The application wants the analysis for its own visualizer view (in addition to a host tap).
    viz_capture: bool,
    /// Summaries made since the application last took them.
    viz_out: Vec<rvp_host::VizSummary>,
    /// The mono waveform of the audio heard last (for an oscilloscope), newest samples at the end.
    scope: Vec<f32>,
    next: Option<NextItem>,
    /// The audio feed has moved on to the next item; the current one is only being heard out.
    feed_next: bool,
    /// Crossfade and automatic level, as the user set them.
    settings: AudioSettings,
    /// What the caller knows about the loudness of the item being played.
    hint: Option<LoudnessTags>,
    /// The crossfade being mixed, if any.
    fade: Option<FadeState>,
    /// The loudness last given to the audio pipeline for the current item (`None`: nothing given yet).
    known_cur: Option<Option<f32>>,
    /// The level settings last given to the audio pipeline.
    level_pushed: Option<LevelConfig>,
}

impl Session {
    /// Start opening `source`. Nothing happens until [`Session::tick`] is called.
    pub fn new<S: Source + 'static>(source: S, codecs: Rc<dyn CodecFactory>) -> Self {
        let (exec, sh) = spawn_item(source, &codecs);
        Self {
            exec,
            sh,
            clock: MasterClock::new(ClockSource::Monotonic),
            audio: None,
            audio_opened: false,
            sink_paused: true,
            want_play: false,
            running: false,
            needs_sink_flush: false,
            volume: 1.0,
            muted: false,
            rate: 1.0,
            ended: false,
            trace: false,
            seek_target: None,
            stats: VideoStats::default(),
            video_trace: None,
            shown_preview: false,
            last_presented: None,
            events: VecDeque::new(),
            shown_subtitle: None,
            shown_cues: Vec::new(),
            steps: VecDeque::new(),
            stepped: false,
            ab_loop: None,
            codecs,
            tag: 0,
            item_no: 0,
            next: None,
            feed_next: false,
            settings: AudioSettings::default(),
            hint: None,
            fade: None,
            known_cur: None,
            level_pushed: None,
            viz: None,
            viz_reset: false,
            viz_capture: false,
            viz_out: Vec::new(),
            scope: Vec::new(),
        }
    }

    /// Switch the analysis for the application's own visualizer on or off. While on, [`Session::take_viz`] returns what the
    /// audio being heard looked like (the host's tap, if it has one, gets the same numbers).
    pub fn set_viz_capture(&mut self, on: bool) {
        self.viz_capture = on;
        if !on {
            self.viz_out.clear();
            self.scope.clear();
        }
    }

    /// The summaries computed since the last call, oldest first, and the latest mono samples heard (up to 2048).
    pub fn take_viz(&mut self) -> (Vec<rvp_host::VizSummary>, &[f32]) {
        (core::mem::take(&mut self.viz_out), &self.scope)
    }

    /// Give the current item a caller-chosen tag (reported back in [`SessionEvent::ItemStarted`] for later ones).
    pub fn set_tag(&mut self, tag: u32) {
        self.tag = tag;
    }

    /// The tag of the item being played.
    pub fn tag(&self) -> u32 {
        self.tag
    }

    /// Open `source` in the background as the item to play after this one. It decodes ahead while the current
    /// item plays, and when the current item's audio ends the next one's follows without a gap. Replaces any
    /// item queued before.
    pub fn queue_next<S: Source + 'static>(&mut self, source: S, tag: u32) {
        self.queue_next_with(source, tag, None);
    }

    /// [`Session::queue_next`] with what the caller knows about the item's loudness (the library's measurement), which is
    /// used when the file's own tags have none.
    pub fn queue_next_with<S: Source + 'static>(&mut self, source: S, tag: u32, hint: Option<LoudnessTags>) {
        let (exec, sh) = spawn_item(source, &self.codecs);
        self.next = Some(NextItem { exec, sh, tag, hint });
        self.feed_next = false;
    }

    /// Set crossfade and automatic level. They take effect at once for the audio still to come.
    pub fn set_audio_settings(&mut self, settings: AudioSettings) {
        self.settings = settings.clamped();
    }

    /// The crossfade and level settings in force.
    pub fn audio_settings(&self) -> AudioSettings {
        self.settings
    }

    /// What the caller knows about the loudness of the item being played (the library's measurement); the file's own tags
    /// come first.
    pub fn set_loudness_hint(&mut self, hint: Option<LoudnessTags>) {
        self.hint = hint;
        self.known_cur = None;
    }

    /// The loudness the automatic level goes by for the item being played, LUFS, when its tags or the library say; `None` while
    /// it is being estimated from the sound.
    pub fn known_loudness(&self) -> Option<f32> {
        let s = self.sh.borrow();
        if !s.opened {
            return None;
        }
        known_loudness(self.settings.level_mode, &s.meta.loudness, self.hint.as_ref())
    }

    /// The gain the automatic level applies now, dB (0 when it is off).
    pub fn level_gain_db(&self) -> f32 {
        self.audio.as_ref().map_or(0.0, |a| a.current_gain_db())
    }

    /// True while one item is being crossfaded into the next.
    pub fn crossfading(&self) -> bool {
        self.fade.is_some()
    }

    /// True if a next item is queued.
    pub fn has_next(&self) -> bool {
        self.next.is_some()
    }

    /// Drop the queued next item.
    pub fn cancel_next(&mut self) {
        if self.feed_next {
            return; // too late: its audio is already joined to ours
        }
        self.next = None;
    }

    /// True when it is time for the caller to queue the next item: the current one is about to run out of data.
    pub fn wants_next(&self, now_us: Timestamp) -> bool {
        if self.next.is_some() || self.ended {
            return false;
        }
        let s = self.sh.borrow();
        if !s.opened {
            return false;
        }
        let lead = NEXT_LEAD_US
            + if self.settings.crossfade { self.settings.crossfade_secs as i64 * 1_000_000 } else { 0 };
        let near_end = s.duration_us.is_some_and(|d| d - self.clock.now_stream(now_us).max(0) < lead);
        near_end || s.demux_done
    }

    /// Loop between `a` and `b` (stream microseconds, `a < b`), or clear the loop with `None`.
    pub fn set_loop(&mut self, range: Option<(Timestamp, Timestamp)>) {
        self.ab_loop = range.filter(|(a, b)| a < b);
    }

    /// The active A-B loop.
    pub fn loop_range(&self) -> Option<(Timestamp, Timestamp)> {
        self.ab_loop
    }

    /// Show the next (`forward`) or previous frame and stay paused. Playing is paused first by the caller.
    pub fn step_frame(&mut self, forward: bool) {
        if self.steps.len() < 8 {
            self.steps.push_back(if forward { 1 } else { -1 });
        }
    }

    /// Title, artist, album and cover art from the container.
    pub fn metadata(&self) -> rvp_core::Metadata {
        self.sh.borrow().meta.clone()
    }

    /// Chapter marks from the container, in time order.
    pub fn chapters(&self) -> Vec<rvp_core::Chapter> {
        self.sh.borrow().chapters.clone()
    }

    /// Next pending event, if any.
    pub fn poll_event(&mut self) -> Option<SessionEvent> {
        self.events.pop_front()
    }

    /// Subtitle tracks: container tracks that we can decode, then sidecar files.
    pub fn subtitle_tracks(&self) -> Vec<SubtitleTrack> {
        self.sh.borrow().subs.iter().map(|t| t.track.clone()).collect()
    }

    /// The selected subtitle track, `None` when subtitles are off.
    pub fn selected_subtitle(&self) -> Option<u32> {
        self.sh.borrow().sel_sub
    }

    /// Select a subtitle track, or `None` to turn subtitles off.
    pub fn select_subtitle(&mut self, id: Option<u32>) {
        let mut s = self.sh.borrow_mut();
        s.sel_sub = id.filter(|i| s.subs.iter().any(|t| t.track.id == *i));
    }

    /// The text of the selected subtitle track at stream time `t`.
    pub fn subtitle_text_at(&self, t: Timestamp) -> Option<String> {
        join_text(&self.subtitle_cues_at(t))
    }

    /// The cues of the selected subtitle track on screen at stream time `t`, in drawing order (text with its ASS
    /// styling, or a PGS picture).
    pub fn subtitle_cues_at(&self, t: Timestamp) -> Vec<Cue> {
        let s = self.sh.borrow();
        let Some(id) = s.sel_sub else { return Vec::new() };
        match s.subs.iter().find(|x| x.track.id == id) {
            Some(slot) => slot.cues.cues_at(t).into_iter().cloned().collect(),
            None => Vec::new(),
        }
    }

    /// The cues on screen as of the last tick ([`Session::subtitle_cues_at`] at that time).
    pub fn subtitle_cues(&self) -> &[Cue] {
        &self.shown_cues
    }

    /// The subtitle text currently on screen (as of the last tick).
    pub fn subtitle_text(&self) -> Option<&str> {
        self.shown_subtitle.as_deref()
    }

    /// Load a sidecar subtitle file (SRT or WebVTT) as a new track and select it once it is parsed. Returns the
    /// track id. The file is read by a task of this session, so it may arrive a little later.
    pub fn add_subtitle_source<S: Source + 'static>(&mut self, mut source: S, name: &str) -> u32 {
        let id = {
            let mut s = self.sh.borrow_mut();
            let id = EXTERNAL_TRACK_BASE + s.next_external;
            s.next_external += 1;
            s.subs.push(SubSlot {
                track: SubtitleTrack { id, language: None, label: String::from(name), external: true },
                cues: CueList::new(),
                kind: SubKind::File,
            });
            id
        };
        let sh = self.sh.clone();
        let name = String::from(name);
        self.exec.spawn(async move {
            let mut data: Vec<u8> = Vec::new();
            let mut buf = alloc::vec![0u8; 64 * 1024];
            while data.len() < MAX_SUB_FILE {
                match source.read_at(data.len() as u64, &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => data.extend_from_slice(&buf[..n]),
                }
            }
            let text = String::from_utf8_lossy(&data);
            let cues = rvp_subs::parse(&text);
            let mut s = sh.borrow_mut();
            if let Some(slot) = s.subs.iter_mut().find(|t| t.track.id == id) {
                if cues.is_empty() {
                    slot.track.label = alloc::format!("{name} (no cues)");
                }
                slot.cues = CueList::from_cues(cues);
            }
            // The new file is what the user just asked for.
            s.sel_sub = Some(id);
            s.progress += 1;
        });
        id
    }

    /// Audio tracks in the container.
    pub fn audio_tracks(&self) -> Vec<StreamInfo> {
        self.sh.borrow().streams.iter().filter(|i| i.kind == StreamKind::Audio).cloned().collect()
    }

    /// Switch to another audio track and resume from the current position (a short rebuffer).
    pub fn select_audio(&mut self, id: u32, now_us: Timestamp) {
        let pos = self.position_us(now_us);
        {
            let mut s = self.sh.borrow_mut();
            let Some(info) = s.streams.iter().find(|i| i.id == id && i.kind == StreamKind::Audio).cloned()
            else {
                return;
            };
            if s.sel_audio.as_ref().is_some_and(|a| a.id == id) {
                return;
            }
            s.sel_audio = Some(info);
            s.audio_epoch = s.audio_epoch.wrapping_add(1);
            s.warnings.retain(|w| !w.starts_with("audio disabled"));
        }
        self.seek(pos);
    }

    /// Record the audio chunks written (tests).
    pub fn enable_audio_trace(&mut self) {
        self.trace = true;
        if let Some(a) = &mut self.audio {
            a.enable_trace();
        }
    }

    /// Audio chunks recorded so far (see [`Session::enable_audio_trace`]).
    pub fn audio_trace(&self) -> &[TraceEntry] {
        self.audio.as_ref().map_or(&[], |a| a.trace())
    }

    /// Record every presented video frame (tests).
    pub fn enable_video_trace(&mut self) {
        self.video_trace = Some(Vec::new());
    }

    /// Presented frames recorded so far.
    pub fn video_trace(&self) -> &[VideoTraceEntry] {
        self.video_trace.as_deref().unwrap_or(&[])
    }

    /// Video presentation counters.
    pub fn video_stats(&self) -> &VideoStats {
        &self.stats
    }

    /// Non-fatal problems (a stream whose codec is unsupported, packets that failed to decode).
    pub fn warnings(&self) -> Vec<String> {
        self.sh.borrow().warnings.clone()
    }

    /// Current state.
    pub fn state(&self) -> SessionState {
        let s = self.sh.borrow();
        if s.error.is_some() && !s.opened {
            SessionState::Failed
        } else if !s.opened {
            SessionState::Opening
        } else if self.ended {
            SessionState::Ended
        } else if self.running {
            SessionState::Playing
        } else if self.want_play {
            SessionState::Buffering
        } else {
            SessionState::Paused
        }
    }

    /// The last error, if any.
    pub fn error(&self) -> Option<Error> {
        self.sh.borrow().error.clone()
    }

    /// Streams in the container (empty until open).
    pub fn streams(&self) -> Vec<StreamInfo> {
        self.sh.borrow().streams.clone()
    }

    /// Duration if known.
    pub fn duration_us(&self) -> Option<Timestamp> {
        self.sh.borrow().duration_us
    }

    /// Start or resume playback.
    pub fn play(&mut self) {
        if self.ended {
            self.ended = false;
            self.seek(0);
        } else if self.stepped && !self.running {
            // Frame steps moved the picture without audio: restart everything from where the picture is.
            self.stepped = false;
            let pos = self.clock.now_stream(0).max(0);
            self.seek(pos);
        }
        self.want_play = true;
    }

    /// Pause. The position freezes.
    pub fn pause(&mut self, now_us: Timestamp) {
        self.want_play = false;
        if self.running {
            self.clock.pause(now_us);
            self.running = false;
        }
    }

    /// Set output volume, 0.0..=1.0.
    pub fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
    }

    /// Mute or unmute.
    pub fn set_muted(&mut self, m: bool) {
        self.muted = m;
    }

    /// Current volume.
    pub fn volume(&self) -> f32 {
        self.volume
    }

    /// True if muted.
    pub fn muted(&self) -> bool {
        self.muted
    }

    /// Playback rate (1.0 = normal).
    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// True if a video stream is selected for decoding (or the file is still opening).
    pub fn has_video(&self) -> bool {
        let s = self.sh.borrow();
        !s.opened || s.sel_video.is_some()
    }

    /// True if the container has a video stream, even one we cannot decode yet.
    pub fn container_has_video(&self) -> bool {
        self.sh.borrow().streams.iter().any(|i| i.kind == StreamKind::Video)
    }

    /// Track id of the selected audio stream.
    pub fn selected_audio(&self) -> Option<u32> {
        self.sh.borrow().sel_audio.as_ref().map(|i| i.id)
    }

    /// Set the playback rate, 0.25..=4.0. Audio is resampled (varispeed), so pitch follows speed until M8's
    /// time-stretching. The pipeline restarts at the current position, which costs a short rebuffer.
    pub fn set_rate(&mut self, rate: f64, now_us: Timestamp) {
        let rate = rate.clamp(0.25, 4.0);
        if (rate - self.rate).abs() < 1e-9 {
            return;
        }
        let pos = self.position_us(now_us);
        self.rate = rate;
        self.clock.set_rate(rate, now_us);
        self.seek(pos);
        if let Some(a) = &mut self.audio {
            a.set_rate(rate);
        }
    }

    /// Playback position at host time `now_us`.
    pub fn position_us(&self, now_us: Timestamp) -> Timestamp {
        self.clock.now_stream(now_us).max(0)
    }

    /// Seek to `target_us`. Playback restarts (through `Buffering`) once audio is available at the target.
    pub fn seek(&mut self, target_us: Timestamp) {
        let target = target_us.max(0);
        let mut s = self.sh.borrow_mut();
        // Audio-only: start a little early so decoders warm up, then discard up to the target.
        let demux_target = if s.sel_audio.is_some() { (target - PREROLL_US).max(0) } else { target };
        s.generation = s.generation.wrapping_add(1);
        s.audio_in.clear();
        s.audio_dec.clear();
        s.audio_dec_us = 0;
        s.audio_done = false;
        s.video_in.clear();
        s.video_dec.clear();
        s.video_done = false;
        s.video_floor = target;
        s.demux_done = false;
        s.seek = Some(demux_target);
        s.seeking = true;
        drop(s);
        self.viz_reset = true;
        if self.feed_next {
            // Seeking out of the tail of an item: the joined next item is dropped (the caller queues it again).
            self.next = None;
            self.feed_next = false;
        }
        self.fade = None;
        if let Some(a) = &mut self.audio {
            a.reset(target);
        }
        self.needs_sink_flush = true;
        self.ended = false;
        self.running = false;
        self.clock.seek(target, 0);
        self.clock.pause(0);
        self.seek_target = Some(target);
        self.shown_preview = false;
        self.last_presented = None;
    }
}

impl Session {
    /// Whether the audio is running short: less than `AUDIO_FEED_US` decoded, waiting or in the sink, and more of it still to come.
    fn audio_hungry<H: Host>(&self, host: &mut H) -> bool {
        let s = self.sh.borrow();
        if s.sel_audio.is_none() || s.audio_done || s.seeking {
            return false;
        }
        let mut us = s.audio_dec_us;
        if let Some(out) = &self.audio {
            let rate = out.sink_rate().max(1) as i64;
            us += (out.pending_frames() as i64 + host.audio().queued_frames() as i64) * 1_000_000 / rate;
        }
        us < AUDIO_FEED_US
    }

    /// Advance the session: run tasks, feed the audio sink, update the clock. Call this regularly (the host
    /// clock's `request_wake` tells the host when).
    pub fn tick<H: Host>(&mut self, host: &mut H) {
        let t0 = host.clock().now_us();

        self.sh.borrow_mut().now = t0;
        if let Some(n) = &self.next {
            n.sh.borrow_mut().now = t0;
        }
        // 1. Demux and decode tasks, until quiescent or out of budget. Pictures only while the budget lasts; after it, rounds for the audio
        // alone (see `AUDIO_ONLY_US`).
        let mut over_at: Option<Timestamp> = None;
        for _ in 0..MAX_ROUNDS {
            let before = self.sh.borrow().progress;
            let next_before = self.next.as_ref().map(|n| n.sh.borrow().progress);
            let now_r = host.clock().now_us();
            let in_budget = now_r - t0 <= BUDGET_US;
            let low = self.audio_hungry(host);
            let clock_us = self.clock.now_stream(now_r);
            {
                let mut sh = self.sh.borrow_mut();
                sh.video_ok = in_budget;
                sh.audio_low = low;
                sh.clock_us = clock_us;
            }
            if let Some(n) = &self.next {
                n.sh.borrow_mut().video_ok = in_budget;
            }
            self.exec.poll_all();
            if let Some(n) = &mut self.next {
                n.exec.poll_all();
            }
            if let Some(e) = self.fade.as_mut().and_then(|f| f.old_exec.as_mut()) {
                e.poll_all();
            }
            let moved = self.sh.borrow().progress != before
                || self.next.as_ref().map(|n| n.sh.borrow().progress) != next_before;
            if !moved {
                break;
            }
            let after = host.clock().now_us();
            if after - t0 > BUDGET_US {
                // Out of budget: go on only while the sound is short of audio, and only for a little longer.
                let over = *over_at.get_or_insert(after);
                if after - over > AUDIO_ONLY_US || !self.audio_hungry(host) {
                    break;
                }
            }
        }
        self.sh.borrow_mut().video_ok = true;
        if let Some(n) = &self.next {
            n.sh.borrow_mut().video_ok = true;
        }
        let now = host.clock().now_us();
        // A queued item that cannot be opened is dropped, and the caller told.
        let failed = self.next.as_ref().is_some_and(|n| {
            let s = n.sh.borrow();
            s.error.is_some() && !s.opened
        });
        if failed {
            if let Some(n) = self.next.take() {
                self.feed_next = false;
                let error =
                    n.sh.borrow().error.clone().unwrap_or(Error::Invalid(String::from("open failed")));
                self.events.push_back(SessionEvent::ItemFailed { tag: n.tag, error });
            }
        }

        // 2. Open the audio sink once we know the streams.
        let (opened, has_audio, seeking) = {
            let s = self.sh.borrow();
            (s.opened, s.sel_audio.is_some(), s.seeking)
        };
        if opened && !self.audio_opened {
            self.audio_opened = true;
            if has_audio && self.audio.is_none() {
                if let Ok(p) = host.audio().open(WANT_AUDIO) {
                    let mut out = AudioOut::new(p);
                    out.set_tap(host.visualizer().is_some());
                    out.set_rate(self.rate);
                    self.level_pushed = None;
                    if self.trace {
                        out.enable_trace();
                    }
                    // A stream of its own: nothing an earlier item (or an earlier open) left in the device counts.
                    host.audio().flush();
                    host.audio().set_paused(true);
                    self.sink_paused = true;
                    self.audio = Some(out);
                    self.clock.set_source(ClockSource::Audio, now);
                }
            }
        }
        if !has_audio && self.audio.is_some() {
            self.audio = None; // decoder could not be created
            self.clock.set_source(ClockSource::Monotonic, now);
        }
        if self.needs_sink_flush && self.audio_opened {
            self.needs_sink_flush = false;
            if self.audio.is_some() {
                host.audio().flush();
            }
        }

        // 3. Feed the audio sink (see `feed_audio`). The device only runs while the clock does: a pause, a seek or the wait for
        // the start buffer must not let what is queued play on.
        self.sync_sink_pause(host);
        self.feed_audio(host, seeking);

        // 4. Start playback once enough is buffered.
        if self.want_play && !self.running && !self.ended && opened && !seeking {
            let audio_done = self.sh.borrow().audio_done;
            let ready = match &self.audio {
                Some(out) => {
                    let frames = out.pending_frames() + host.audio().queued_frames();
                    let queued_us = frames as i64 * 1_000_000 / WANT_AUDIO.sample_rate as i64;
                    queued_us >= START_BUFFER_US
                        || (audio_done && out.origin().is_some())
                        || (audio_done && frames == 0)
                }
                None => true,
            };
            let ready = ready && {
                let s = self.sh.borrow();
                let floor = self.seek_target.unwrap_or(0);
                s.sel_video.is_none() || s.video_done || s.video_dec.iter().any(|f| f.pts >= floor)
            };
            if ready {
                let start = match &self.audio {
                    Some(out) => out.heard_pts(&*host.audio()).or(self.seek_target),
                    None => None,
                };
                if let Some(t) = start.or(Some(self.clock.now_stream(now))) {
                    self.clock.seek(t, now);
                }
                self.clock.resume(now);
                self.running = true;
                self.sync_sink_pause(host);
            }
        }

        // 5. Keep the clock locked to what is being heard; when the next item's audio is what is heard, it
        // becomes the current item.
        if self.running {
            let mut heard_next = false;
            if let Some(out) = &self.audio {
                if self.feed_next {
                    heard_next = out.heard_item(&*host.audio()) == Some(self.item_no + 1);
                }
                // Once every sample has been heard the audio has nothing more to say about the time: the picture (which
                // may be longer than the sound) goes on by the clock's own count instead of waiting for it.
                let audio_over = {
                    let s = self.sh.borrow();
                    s.audio_done
                        && s.audio_dec.is_empty()
                        && out.pending_frames() == 0
                        && host.audio().queued_frames() == 0
                };
                if !heard_next && !audio_over {
                    if let Some(h) = out.heard_pts(&*host.audio()) {
                        self.clock.update_from_audio(h, now);
                    }
                }
            }
            // The next item may have nothing audible to wait for (its audio never came): move on once drained.
            let stalled = self.feed_next
                && !heard_next
                && self
                    .audio
                    .as_ref()
                    .is_some_and(|out| out.pending_frames() == 0 && host.audio().queued_frames() == 0);
            if heard_next || stalled {
                let pos = self.audio.as_ref().and_then(|o| o.heard_pts(&*host.audio())).unwrap_or(0);
                self.promote(now, Some(pos.max(0)));
            }
            if let Some(out) = &mut self.audio {
                out.prune(&*host.audio());
            }
        }

        // 6. Video: show the frame that is due, dropping any that are already late.
        self.present_video(host, now);

        // 7. End of stream.
        if self.running {
            let s = self.sh.borrow();
            let audio_drained = match &self.audio {
                Some(out) => {
                    s.audio_done
                        && s.audio_dec.is_empty()
                        && out.pending_frames() == 0
                        && host.audio().queued_frames() == 0
                }
                None => s.demux_done,
            };
            let video_drained = s.sel_video.is_none() || (s.video_done && s.video_dec.is_empty());
            drop(s);
            if audio_drained && video_drained && !self.feed_next {
                if self.next.is_some() {
                    // The next item was not ready in time for a gapless join (or has no audio): start it now.
                    self.promote(now, None);
                } else {
                    self.running = false;
                    self.ended = true;
                    self.clock.pause(now);
                    self.want_play = false;
                }
            }
        }

        // 6b. Visualizer tap: hand over what has been heard since the last tick, with its analysis.
        self.feed_visualizer(host);

        // 7b. A-B loop: past B, jump back to A and keep playing.
        if let (true, Some((a, b))) = (self.running, self.ab_loop) {
            if self.clock.now_stream(now) >= b {
                self.seek(a);
            }
        }

        // 8. Subtitles: report changes.
        let pos = self.clock.now_stream(now);
        let cues = self.subtitle_cues_at(pos);
        if cues != self.shown_cues {
            let text = join_text(&cues);
            self.events.push_back(SessionEvent::Subtitle { at_us: pos, text: text.clone() });
            self.shown_subtitle = text;
            self.shown_cues = cues;
        }

        host.clock().request_wake(now + TICK_US);
    }
}

impl Session {
    /// Present at most one video frame: the latest one that is due at the current clock position.
    ///
    /// While running, every frame whose time has come is "due"; all but the newest are counted as dropped
    /// (they would have been on screen for less than a tick). While paused, one preview frame is shown per
    /// seek: the last frame at or before the position, once the following frame (or end of stream) proves
    /// no better candidate is coming.
    fn present_video<H: Host>(&mut self, host: &mut H, now: Timestamp) {
        let pos = self.clock.now_stream(now);
        if !self.steps.is_empty() && !self.running {
            self.run_steps(host, now);
            return;
        }
        let mut s = self.sh.borrow_mut();
        if s.sel_video.is_none() || !s.opened || s.seeking {
            return;
        }
        let mut candidate: Option<VideoFrame> = None;
        let mut dropped = 0u64;
        if self.running || !self.shown_preview {
            let may_show = self.running || s.video_dec.iter().any(|f| f.pts > pos) || s.video_done;
            if may_show {
                while s.video_dec.front().is_some_and(|f| f.pts <= pos) {
                    if candidate.is_some() {
                        dropped += 1;
                    }
                    candidate = s.video_dec.pop_front();
                }
            }
        }
        drop(s);
        // How long the previous frame has been on screen (measured before it is replaced).
        if self.running {
            if let Some(last) = self.last_presented {
                self.stats.max_staleness_us = self.stats.max_staleness_us.max(pos - last);
            }
        }
        self.stats.dropped += dropped;
        if let Some(f) = candidate {
            host.video().present(&f);
            self.stats.presented += 1;
            let drift = (pos - f.pts).abs();
            self.stats.max_drift_us = self.stats.max_drift_us.max(drift);
            if let Some(t) = &mut self.video_trace {
                t.push(VideoTraceEntry { clock_us: pos, pts: f.pts });
            }
            self.last_presented = Some(f.pts);
            self.shown_preview = true;
        }
    }
}

impl Session {
    /// Feed the host's visualizer tap (if it has one) the audio heard since the last call.
    fn feed_visualizer<H: Host>(&mut self, host: &mut H) {
        let want = host.visualizer().is_some() || self.viz_capture;
        let Some(out) = &mut self.audio else { return };
        out.set_tap(want);
        if !want {
            self.viz = None;
            return;
        }
        let params = out.sink_params();
        if self.viz.as_ref().is_none_or(|a| a.sample_rate() != params.sample_rate) {
            self.viz = Some(rvp_viz::Analyzer::new(params.sample_rate));
        }
        if core::mem::take(&mut self.viz_reset) {
            if let Some(a) = &mut self.viz {
                a.reset();
            }
        }
        let Some((pts, pcm)) = out.take_heard(&*host.audio()) else { return };
        let mut summaries = Vec::new();
        if let Some(a) = &mut self.viz {
            a.process(&pcm, params.channels as usize, pts, &mut summaries);
        }
        if self.viz_capture {
            self.viz_out.extend_from_slice(&summaries);
            if self.viz_out.len() > 512 {
                let drop = self.viz_out.len() - 512;
                self.viz_out.drain(..drop);
            }
            let ch = params.channels.max(1) as usize;
            self.scope.extend(pcm.chunks_exact(ch).map(|f| f.iter().sum::<f32>() / ch as f32));
            if self.scope.len() > 2048 {
                let drop = self.scope.len() - 2048;
                self.scope.drain(..drop);
            }
        }
        if let Some(tap) = host.visualizer() {
            tap.push_block(&rvp_host::VizBlock {
                pts_us: pts,
                sample_rate: params.sample_rate,
                channels: params.channels,
                samples: &pcm,
            });
            for s in &summaries {
                tap.push_summary(s);
            }
        }
    }

    /// Feed the audio sink. Once the current item has handed over all its audio and the next one is ready, the feed moves on
    /// to the next item with no gap and no flush: that is gapless playback. With crossfade on, the last seconds of the item and
    /// the first of the next are mixed instead (see `fade_threshold`).
    fn feed_audio<H: Host>(&mut self, host: &mut H, seeking: bool) {
        let Some(mut out) = self.audio.take() else { return };
        self.sync_level(&mut out);
        let handover = !self.feed_next
            && !seeking
            && self.next.as_ref().is_some_and(|n| {
                let cur = self.sh.borrow();
                let nx = n.sh.borrow();
                cur.audio_done && cur.audio_dec.is_empty() && nx.opened && nx.sel_audio.is_some()
            });
        if handover {
            self.feed_next = true;
            let item = self.item_no + 1;
            let known = self.next.as_ref().and_then(|n| {
                let nx = n.sh.borrow();
                known_loudness(self.settings.level_mode, &nx.meta.loudness, n.hint.as_ref())
            });
            out.begin_item(item);
            out.set_item_loudness(known);
            self.known_cur = Some(known);
        }
        let want = (WANT_AUDIO.sample_rate / 5) as usize;
        if !seeking {
            let mut fade_possible = true;
            loop {
                if self.fade.is_some() {
                    self.feed_fade(&mut out, want);
                }
                if self.fade.is_none() {
                    let feed: Sh = match (&self.next, self.feed_next) {
                        (Some(n), true) => n.sh.clone(),
                        _ => self.sh.clone(),
                    };
                    // Feed until the audio pushed reaches the point where the fade is to begin.
                    let due = if fade_possible { self.fade_threshold() } else { None };
                    let mut reached = false;
                    while out.pending_frames() < want {
                        if due.is_some_and(|t| out.end_pts().is_some_and(|e| e >= t)) {
                            reached = true;
                            break;
                        }
                        let Some(b) = pop_audio(&feed) else { break };
                        out.push(b);
                    }
                    if reached {
                        fade_possible = self.start_fade(&mut out);
                        continue;
                    }
                }
                break;
            }
            out.drain(host.audio());
        }
        host.audio().set_volume(if self.muted { 0.0 } else { self.volume });
        self.audio = Some(out);
    }

    /// Keep the device paused exactly while the clock is not running (tracked, so the host is only told on a change).
    fn sync_sink_pause<H: Host>(&mut self, host: &mut H) {
        if self.audio.is_some() && self.sink_paused == self.running {
            self.sink_paused = !self.running;
            host.audio().set_paused(self.sink_paused);
        }
    }

    /// Both items of a crossfade at once: keep each one's share of the mixer stocked and send the mixture on.
    fn feed_fade(&mut self, out: &mut AudioOut, want: usize) {
        let Some(fade) = &self.fade else { return };
        let (a_sh, b_sh) = (fade.a.clone(), fade.b.clone());
        let over = |sh: &Sh| {
            let s = sh.borrow();
            s.audio_done && s.audio_dec.is_empty()
        };
        while out.pending_frames() < want {
            let (fa, fb) = out.fade_frames();
            let mut moved = false;
            if fa < want {
                if let Some(b) = pop_audio(&a_sh) {
                    out.push_fade_a(b);
                    moved = true;
                }
            }
            if fb < want {
                if let Some(b) = pop_audio(&b_sh) {
                    out.push_fade_b(b);
                    moved = true;
                }
            }
            let before = out.pending_frames();
            if out.fade_mix(over(&a_sh), over(&b_sh)) == FadeStatus::Finished {
                self.fade = None;
                return;
            }
            if !moved && out.pending_frames() == before {
                return;
            }
        }
    }

    /// Give the audio pipeline the level settings and the loudness of the item being fed, when they changed.
    fn sync_level(&mut self, out: &mut AudioOut) {
        let cfg =
            LevelConfig { enabled: self.settings.auto_level, target_lufs: self.settings.target_lufs as f32 };
        if self.level_pushed != Some(cfg) {
            self.level_pushed = Some(cfg);
            out.set_level(cfg);
        }
        if self.feed_next || self.fade.is_some() {
            return; // the next item's loudness was given when the hand-over or the fade began
        }
        let known = {
            let s = self.sh.borrow();
            s.opened.then(|| known_loudness(self.settings.level_mode, &s.meta.loudness, self.hint.as_ref()))
        };
        if let Some(known) = known {
            if self.known_cur != Some(known) {
                self.known_cur = Some(known);
                out.set_item_loudness(known);
            }
        }
    }

    /// Where (stream time of the audio fed, microseconds) the crossfade into the queued item is to begin, if there is to be
    /// one: the settings ask for it, both items are plain audio that can be mixed (no picture on either side), they are not
    /// consecutive tracks of a gapless album, and the fade is long enough to hear. The fade is as long as the setting,
    /// shortened to a third of the shorter item; when that leaves less than a quarter of a second the join stays gapless.
    fn fade_threshold(&self) -> Option<Timestamp> {
        if !self.settings.crossfade || self.feed_next || self.fade.is_some() {
            return None;
        }
        let n = self.next.as_ref()?;
        if n.tag == self.tag {
            return None; // repeat one: a track is not faded into itself
        }
        let cur = self.sh.borrow();
        let nx = n.sh.borrow();
        if !cur.opened || !nx.opened || nx.sel_audio.is_none() || cur.sel_audio.is_none() {
            return None;
        }
        let has_video = |s: &Shared| s.streams.iter().any(|i| i.kind == StreamKind::Video);
        if has_video(&cur) || has_video(&nx) || same_gapless_album(&cur.meta, &nx.meta) {
            return None;
        }
        let (d_a, d_b) = (audio_end(&cur)?, audio_end(&nx)?);
        let want = self.settings.crossfade_secs as i64 * 1_000_000;
        let len = want.min(d_a.min(d_b) / FADE_MAX_SHARE);
        (len >= MIN_FADE_US).then_some(d_a - len)
    }

    /// Begin the crossfade into the queued item now (the audio fed has reached [`Session::fade_threshold`]). The fade runs to the
    /// end of this item's audio, at most as long as planned. Returns false if it cannot be made (too little of this item is
    /// left): the join is gapless then.
    fn start_fade(&mut self, out: &mut AudioOut) -> bool {
        let Some(th) = self.fade_threshold() else { return false };
        let Some(n) = &self.next else { return false };
        let d_a = audio_end(&self.sh.borrow()).unwrap_or(0);
        let end = out.end_pts().unwrap_or(th);
        let len = (d_a - end).min(self.settings.crossfade_secs as i64 * 1_000_000);
        if len < MIN_FADE_US {
            return false;
        }
        let known_b = known_loudness(self.settings.level_mode, &n.sh.borrow().meta.loudness, n.hint.as_ref());
        let frames = (len * out.sink_params().sample_rate as i64 / 1_000_000) as u64;
        out.begin_fade(frames, self.item_no + 1, known_b);
        self.fade = Some(FadeState { a: self.sh.clone(), b: n.sh.clone(), old_exec: None });
        self.feed_next = true;
        self.known_cur = Some(known_b);
        true
    }

    /// Make the queued next item the current one. `heard` is the position its audio has reached when the switch
    /// happens during playback (gapless); `None` means the previous item ended and the new one starts from zero.
    fn promote(&mut self, now: Timestamp, heard: Option<Timestamp>) {
        let Some(n) = self.next.take() else { return };
        let old_exec = core::mem::replace(&mut self.exec, n.exec);
        if heard.is_some() {
            // In the middle of a crossfade the first item's tasks are still needed for the rest of it.
            if let Some(f) = &mut self.fade {
                f.old_exec = Some(old_exec);
            }
        }
        self.sh = n.sh;
        self.hint = n.hint;
        if heard.is_none() {
            self.known_cur = None;
        }
        self.tag = n.tag;
        self.item_no += 1;
        self.feed_next = false;
        self.last_presented = None;
        self.seek_target = None;
        self.steps.clear();
        self.stepped = false;
        self.ab_loop = None;
        self.shown_subtitle = None;
        self.shown_cues.clear();
        self.shown_preview = false;
        self.audio_opened = false; // let the sink logic look at the new item's streams
        match heard {
            Some(pos) => {
                self.clock.seek(pos, now);
            }
            None => {
                // Previous item played out: begin the new one like a fresh open.
                if let Some(a) = &mut self.audio {
                    a.reset(0);
                    a.begin_item(self.item_no);
                }
                self.running = false;
                self.want_play = true;
                self.ended = false;
                self.clock.seek(0, now);
                self.clock.pause(now);
            }
        }
        self.events.push_back(SessionEvent::ItemStarted { tag: self.tag });
    }

    /// Carry out pending frame steps while paused. A forward step needs the next decoded frame; a backward step
    /// is an exact seek to just before the frame on screen, which shows the frame before it.
    fn run_steps<H: Host>(&mut self, host: &mut H, now: Timestamp) {
        let (seeking, has_video, opened) = {
            let s = self.sh.borrow();
            (s.seeking, s.sel_video.is_some(), s.opened)
        };
        if seeking || !has_video || !opened {
            if opened && !has_video {
                self.steps.clear();
            }
            return;
        }
        // A step only makes sense once the picture for the current position is up.
        if !self.shown_preview {
            let pos = self.clock.now_stream(now);
            let s = self.sh.borrow_mut();
            let may_show = s.video_dec.iter().any(|f| f.pts > pos) || s.video_done;
            if !may_show {
                return;
            }
            drop(s);
            self.show_preview(host, pos);
            return;
        }
        let Some(&dir) = self.steps.front() else { return };
        let cur = self.last_presented.unwrap_or(0);
        if dir > 0 {
            let frame = {
                let mut s = self.sh.borrow_mut();
                while s.video_dec.front().is_some_and(|f| f.pts <= cur) {
                    s.video_dec.pop_front();
                }
                match s.video_dec.pop_front() {
                    Some(f) => Some(f),
                    None => {
                        if s.video_done {
                            self.steps.pop_front();
                        }
                        None
                    }
                }
            };
            if let Some(f) = frame {
                self.steps.pop_front();
                let pts = f.pts;
                host.video().present(&f);
                self.stats.presented += 1;
                self.last_presented = Some(pts);
                self.clock.seek(pts, now);
                self.clock.pause(now);
                self.stepped = true;
                if let Some(t) = &mut self.video_trace {
                    t.push(VideoTraceEntry { clock_us: pts, pts });
                }
            }
        } else {
            self.steps.pop_front();
            if cur > 0 {
                self.seek((cur - 1).max(0));
                self.stepped = true;
            }
        }
    }

    /// Show the last decoded frame at or before `pos` (the preview after a seek).
    fn show_preview<H: Host>(&mut self, host: &mut H, pos: Timestamp) {
        let mut s = self.sh.borrow_mut();
        let mut candidate = None;
        while s.video_dec.front().is_some_and(|f| f.pts <= pos) {
            candidate = s.video_dec.pop_front();
        }
        drop(s);
        if let Some(f) = candidate {
            host.video().present(&f);
            self.stats.presented += 1;
            self.last_presented = Some(f.pts);
            self.shown_preview = true;
        } else {
            self.shown_preview = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(album: &str, track: Option<u32>, disc: Option<u32>, gapless: bool) -> Metadata {
        Metadata {
            album: (!album.is_empty()).then(|| String::from(album)),
            track,
            disc,
            gapless_album: gapless,
            ..Metadata::default()
        }
    }

    #[test]
    fn gapless_albums_are_recognised_by_flag_album_and_adjacent_numbers() {
        let a = |t| meta("Live", Some(t), None, true);
        assert!(same_gapless_album(&a(1), &a(2)));
        assert!(!same_gapless_album(&a(1), &a(3)), "not consecutive: a shuffled queue is faded");
        assert!(!same_gapless_album(&a(2), &a(1)));
        // The flag has to be on both, and the album the same (case and spaces do not matter).
        assert!(!same_gapless_album(&a(1), &meta("Live", Some(2), None, false)));
        assert!(!same_gapless_album(&a(1), &meta("Other", Some(2), None, true)));
        assert!(same_gapless_album(&a(1), &meta(" live ", Some(2), None, true)));
        assert!(
            !same_gapless_album(&meta("", Some(1), None, true), &meta("", Some(2), None, true)),
            "no album, no claim"
        );
        // A disc change counts as consecutive when the next disc starts at track 1; missing numbers are taken as consecutive.
        assert!(same_gapless_album(&meta("L", Some(12), Some(1), true), &meta("L", Some(1), Some(2), true)));
        assert!(!same_gapless_album(&meta("L", Some(12), Some(1), true), &meta("L", Some(2), Some(2), true)));
        assert!(same_gapless_album(&meta("L", None, None, true), &meta("L", Some(7), None, true)));
    }

    #[test]
    fn loudness_comes_from_tags_then_the_library_and_the_mode_picks_track_or_album() {
        let tags = LoudnessTags { track_lufs: Some(-10.0), album_lufs: Some(-12.0), ..Default::default() };
        let hint = LoudnessTags { track_lufs: Some(-20.0), album_lufs: Some(-22.0), ..Default::default() };
        let none = LoudnessTags::default();
        assert_eq!(
            known_loudness(LevelMode::Track, &tags, Some(&hint)),
            Some(-10.0),
            "the file's tags first"
        );
        assert_eq!(known_loudness(LevelMode::Album, &tags, Some(&hint)), Some(-12.0));
        assert_eq!(known_loudness(LevelMode::Track, &none, Some(&hint)), Some(-20.0), "then the library");
        assert_eq!(known_loudness(LevelMode::Album, &none, Some(&hint)), Some(-22.0));
        let track_only = LoudnessTags { track_lufs: Some(-9.0), ..Default::default() };
        assert_eq!(
            known_loudness(LevelMode::Album, &track_only, None),
            Some(-9.0),
            "no album figure: the track's own"
        );
        assert_eq!(
            known_loudness(LevelMode::Track, &none, None),
            None,
            "nothing known: estimate while playing"
        );
        assert_eq!(known_loudness(LevelMode::Album, &none, None), None);
    }

    #[test]
    fn state_machine_without_a_file_reports_failure() {
        use alloc::string::String;
        use rvp_host::mock::MemSource;
        struct NoCodecs;
        impl CodecFactory for NoCodecs {
            fn audio(
                &self,
                _: &StreamInfo,
            ) -> rvp_core::Result<alloc::boxed::Box<dyn rvp_core::AudioDecoder>> {
                Err(Error::Unsupported(String::from("none")))
            }
            fn video(
                &self,
                _: &StreamInfo,
            ) -> rvp_core::Result<alloc::boxed::Box<dyn rvp_core::VideoDecoder>> {
                Err(Error::Unsupported(String::from("none")))
            }
        }
        let mut s = Session::new(MemSource::new(alloc::vec![0u8; 64]), Rc::new(NoCodecs));
        assert_eq!(s.state(), SessionState::Opening);
        // No host needed to poll tasks: the open fails on garbage.
        for _ in 0..4 {
            s.exec.poll_all();
        }
        assert_eq!(s.state(), SessionState::Failed);
        assert!(matches!(s.error(), Some(Error::Unsupported(_))));
    }
}
