//! Audio output: a `cpal` stream fed from a ring buffer, and a silent stand-in driven by the wall clock when there is no device.
//!
//! The player writes decoded samples into the ring from the UI thread; the device callback takes them out. `queued_frames` is what is
//! in the ring and `output_latency_us` is the device's own delay (what `cpal` predicts between the callback and the sound leaving),
//! which together are what the player needs to keep video in step with what is heard.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};
use rvp_core::{AudioParams, Timestamp};
use rvp_host::{AudioSink, HostError};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// A second of audio at most is held between the player and the device.
const RING_SECONDS: usize = 1;

struct Ring {
    buf: Vec<f32>,
    /// Read and write positions in samples, counting up forever (the buffer is indexed modulo its length).
    read: u64,
    write: u64,
}

impl Ring {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(1)], read: 0, write: 0 }
    }

    fn len(&self) -> usize {
        (self.write - self.read) as usize
    }

    fn free(&self) -> usize {
        self.buf.len() - self.len()
    }

    fn push(&mut self, data: &[f32]) {
        let n = self.buf.len() as u64;
        for &s in data {
            self.buf[(self.write % n) as usize] = s;
            self.write += 1;
        }
    }

    /// Copy up to `out.len()` samples out; returns how many.
    fn pop_into(&mut self, out: &mut [f32]) -> usize {
        let n = self.buf.len() as u64;
        let take = self.len().min(out.len());
        for o in out.iter_mut().take(take) {
            *o = self.buf[(self.read % n) as usize];
            self.read += 1;
        }
        take
    }

    fn clear(&mut self) {
        self.read = self.write;
    }
}

/// State shared with the device callback.
struct Shared {
    ring: Mutex<Ring>,
    paused: AtomicBool,
    volume: AtomicU32,
    /// Device delay in microseconds as last predicted by the backend.
    latency_us: AtomicI64,
    /// Frames the device has taken (played), for the report.
    played_frames: AtomicU64,
    /// The stream reported an error (device unplugged).
    failed: AtomicBool,
}

/// Where the sound goes.
enum Output {
    /// Not opened yet.
    Closed,
    /// A device.
    Device { _stream: Stream, name: String },
    /// No device: the ring is drained in wall-clock time so playback still runs (video, seeking, the clock).
    Silent { last: Instant },
}

/// The audio sink of the desktop host.
pub struct DesktopAudio {
    shared: Arc<Shared>,
    out: Output,
    params: Option<AudioParams>,
    /// Channels of the stream when the device has more than the player writes.
    device_channels: u16,
    force_silent: bool,
    written: u64,
    silent_carry: f64,
}

/// The ring's lock, whatever happened to the thread that held it last: the ring is plain numbers, so what a panic left is still usable, and
/// neither the device's callback nor the player's thread may panic because of another thread's failure.
fn lock_ring(m: &Mutex<Ring>) -> std::sync::MutexGuard<'_, Ring> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl DesktopAudio {
    /// A closed sink; `silent` never touches a device.
    pub fn new(silent: bool) -> Self {
        Self {
            shared: Arc::new(Shared {
                ring: Mutex::new(Ring::new(1)),
                paused: AtomicBool::new(true),
                volume: AtomicU32::new(1.0f32.to_bits()),
                latency_us: AtomicI64::new(40_000),
                played_frames: AtomicU64::new(0),
                failed: AtomicBool::new(false),
            }),
            out: Output::Closed,
            params: None,
            device_channels: 2,
            force_silent: silent,
            written: 0,
            silent_carry: 0.0,
        }
    }

    /// What the sound goes to: the device's name, or `none`.
    pub fn backend(&self) -> String {
        match &self.out {
            Output::Device { name, .. } => format!("cpal:{name}"),
            Output::Silent { .. } => "none".into(),
            Output::Closed => "closed".into(),
        }
    }

    /// Frames accepted since the device was opened.
    pub fn frames_written(&self) -> u64 {
        self.written
    }

    /// Frames the device (or the silent clock) has consumed.
    pub fn frames_played(&self) -> u64 {
        self.shared.played_frames.load(Ordering::Relaxed)
    }

    /// True if the device reported an error since it was opened.
    pub fn failed(&self) -> bool {
        self.shared.failed.load(Ordering::Relaxed)
    }

    fn ring_sized(&self, frames: usize, channels: usize) {
        *lock_ring(&self.shared.ring) = Ring::new(frames * channels * RING_SECONDS);
    }

    fn try_device(&mut self, want: AudioParams) -> Result<AudioParams, String> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("no audio output device")?;
        let name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "default".into());
        let default = device.default_output_config().map_err(|e| e.to_string())?;
        // Prefer exactly what the player has (its rate and channel count), else the device's default.
        let mut chosen: Option<(u32, u16)> = None;
        if let Ok(configs) = device.supported_output_configs() {
            for c in configs {
                if c.channels() == want.channels
                    && c.min_sample_rate() <= want.sample_rate
                    && c.max_sample_rate() >= want.sample_rate
                {
                    chosen = Some((want.sample_rate, want.channels));
                    break;
                }
            }
        }
        let (rate, channels) = chosen.unwrap_or((default.sample_rate(), default.channels()));
        let config = StreamConfig { channels, sample_rate: rate, buffer_size: cpal::BufferSize::Default };
        // The callback converts from f32 to whatever the device likes.
        let format = default.sample_format();
        let player_channels = want.channels.min(channels).max(1);
        self.device_channels = channels;
        self.ring_sized(rate as usize, player_channels as usize);
        let stream = match format {
            SampleFormat::F32 => self.build::<f32>(&device, config, player_channels),
            SampleFormat::I16 => self.build::<i16>(&device, config, player_channels),
            SampleFormat::U16 => self.build::<u16>(&device, config, player_channels),
            SampleFormat::I32 => self.build::<i32>(&device, config, player_channels),
            SampleFormat::U8 => self.build::<u8>(&device, config, player_channels),
            SampleFormat::I8 => self.build::<i8>(&device, config, player_channels),
            SampleFormat::F64 => self.build::<f64>(&device, config, player_channels),
            other => Err(format!("unsupported sample format {other}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        self.out = Output::Device { _stream: stream, name };
        Ok(AudioParams { sample_rate: rate, channels: player_channels })
    }

    fn build<T>(
        &self,
        device: &cpal::Device,
        config: StreamConfig,
        player_channels: u16,
    ) -> Result<Stream, String>
    where
        T: SizedSample + FromSample<f32> + Send + 'static,
    {
        let shared = self.shared.clone();
        let err_shared = self.shared.clone();
        let device_channels = config.channels as usize;
        let pc = player_channels as usize;
        let rate = config.sample_rate as f64;
        let mut scratch: Vec<f32> = Vec::new();
        device
            .build_output_stream(
                config,
                move |out: &mut [T], info: &cpal::OutputCallbackInfo| {
                    let ts = info.timestamp();
                    if let Some(d) = ts.playback.checked_duration_since(ts.callback) {
                        let us = d.as_micros() as i64;
                        // Smooth it: the backends report a jittery estimate.
                        let old = shared.latency_us.load(Ordering::Relaxed);
                        shared.latency_us.store((old * 7 + us) / 8, Ordering::Relaxed);
                    }
                    let frames = out.len() / device_channels.max(1);
                    if shared.paused.load(Ordering::Relaxed) {
                        out.fill(T::from_sample(0.0f32));
                        return;
                    }
                    scratch.resize(frames * pc, 0.0);
                    let got = lock_ring(&shared.ring).pop_into(&mut scratch) / pc.max(1);
                    let vol = f32::from_bits(shared.volume.load(Ordering::Relaxed));
                    for f in 0..frames {
                        for c in 0..device_channels {
                            let v = if f < got && c < pc { scratch[f * pc + c] * vol } else { 0.0 };
                            out[f * device_channels + c] = T::from_sample(v.clamp(-1.0, 1.0));
                        }
                    }
                    shared.played_frames.fetch_add(got as u64, Ordering::Relaxed);
                    let _ = rate;
                },
                move |_err| {
                    err_shared.failed.store(true, Ordering::Relaxed);
                },
                None,
            )
            .map_err(|e| e.to_string())
    }

    /// Drain the silent clock's ring according to the wall clock.
    fn drain_silent(&mut self) {
        let Output::Silent { last } = &mut self.out else { return };
        let now = Instant::now();
        let dt = now.duration_since(*last).as_secs_f64();
        *last = now;
        if self.shared.paused.load(Ordering::Relaxed) {
            return;
        }
        let Some(p) = self.params else { return };
        self.silent_carry += dt * p.sample_rate as f64;
        let n = self.silent_carry as usize;
        self.silent_carry -= n as f64;
        let mut ring = lock_ring(&self.shared.ring);
        let take = (n * p.channels as usize).min(ring.len());
        ring.read += take as u64;
        self.shared.played_frames.fetch_add((take / p.channels.max(1) as usize) as u64, Ordering::Relaxed);
    }
}

impl AudioSink for DesktopAudio {
    fn open(&mut self, want: AudioParams) -> Result<AudioParams, HostError> {
        self.out = Output::Closed;
        self.written = 0;
        let got = if self.force_silent { Err("audio is off".to_string()) } else { self.try_device(want) };
        let params = match got {
            Ok(p) => p,
            Err(e) => {
                if !self.force_silent {
                    eprintln!("rusty-wave: no sound ({e}); playing silently");
                }
                self.ring_sized(want.sample_rate as usize, want.channels as usize);
                self.out = Output::Silent { last: Instant::now() };
                want
            }
        };
        self.params = Some(params);
        self.silent_carry = 0.0;
        self.shared.failed.store(false, Ordering::Relaxed);
        Ok(params)
    }

    fn queued_frames(&self) -> usize {
        let ch = self.params.map_or(1, |p| p.channels.max(1) as usize);
        let mut queued = lock_ring(&self.shared.ring).len();
        if let (Output::Silent { last }, Some(p)) = (&self.out, self.params) {
            // No device: what the wall clock has played since the ring was last drained is gone too.
            if !self.shared.paused.load(Ordering::Relaxed) {
                let gone =
                    (last.elapsed().as_secs_f64() * p.sample_rate as f64 + self.silent_carry) as usize * ch;
                queued = queued.saturating_sub(gone);
            }
        }
        queued / ch
    }

    fn output_latency_us(&self) -> Timestamp {
        match self.out {
            Output::Device { .. } => self.shared.latency_us.load(Ordering::Relaxed),
            _ => 0,
        }
    }

    fn write(&mut self, interleaved: &[f32]) -> usize {
        self.drain_silent();
        let Some(p) = self.params else { return 0 };
        let ch = p.channels.max(1) as usize;
        let mut ring = lock_ring(&self.shared.ring);
        let frames = (interleaved.len() / ch).min(ring.free() / ch);
        ring.push(&interleaved[..frames * ch]);
        self.written += frames as u64;
        frames
    }

    fn flush(&mut self) {
        self.drain_silent();
        lock_ring(&self.shared.ring).clear();
    }

    fn set_paused(&mut self, paused: bool) {
        self.drain_silent();
        self.shared.paused.store(paused, Ordering::Relaxed);
        if let Output::Silent { last } = &mut self.out {
            *last = Instant::now();
        }
    }

    fn set_volume(&mut self, volume: f32) {
        self.shared.volume.store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
}

impl DesktopAudio {
    /// Advance the silent clock (called every tick so `queued_frames` stays honest without a device).
    pub fn poll(&mut self) {
        self.drain_silent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_wraps_and_flushes() {
        let mut r = Ring::new(8);
        r.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let mut out = [0.0; 4];
        assert_eq!(r.pop_into(&mut out), 4);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);
        r.push(&[7.0, 8.0, 9.0, 10.0]); // wraps
        assert_eq!(r.len(), 6);
        let mut out = [0.0; 8];
        assert_eq!(r.pop_into(&mut out), 6);
        assert_eq!(&out[..6], &[5.0, 6.0, 7.0, 8.0, 9.0, 10.0]);
        r.push(&[1.0; 5]);
        r.clear();
        assert_eq!((r.len(), r.free()), (0, 8));
    }

    #[test]
    fn the_silent_sink_drains_in_wall_time() {
        let mut a = DesktopAudio::new(true);
        let p = a.open(AudioParams { sample_rate: 1000, channels: 2 }).unwrap();
        assert_eq!(p, AudioParams { sample_rate: 1000, channels: 2 });
        a.set_paused(false);
        assert_eq!(a.write(&vec![0.0; 2 * 400]), 400);
        std::thread::sleep(std::time::Duration::from_millis(150));
        a.poll();
        let q = a.queued_frames();
        assert!((150..=290).contains(&q), "{q}");
        assert_eq!(a.write(&vec![0.0; 2 * 5000]), 1000 - q);
        a.flush();
        assert_eq!(a.queued_frames(), 0);
        assert_eq!(a.backend(), "none");
    }
}

#[cfg(test)]
mod poison_tests {
    use super::*;

    #[test]
    fn a_poisoned_ring_is_still_usable_by_both_threads() {
        let m = Arc::new(Mutex::new(Ring::new(8)));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("a thread dies holding the ring");
        })
        .join();
        assert!(m.lock().is_err(), "poisoned");
        let mut r = lock_ring(&m);
        r.clear();
        assert_eq!(r.len(), 0);
    }
}
