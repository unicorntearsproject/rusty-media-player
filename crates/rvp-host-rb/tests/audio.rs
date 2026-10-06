//! The audio sink and the clock: ring, latency, the derived numbers, devices, and playing a file through the whole chain.
mod common;
use bucket_v0_mock::{MockHost, events};
use bucket_v0_sys::{self as sys, caps, ev};
use common::Harness;
use rvp_core::AudioParams;
use rvp_host::AudioSink;
use rvp_host_rb::audio::RbAudio;

const STEREO_48K: AudioParams = AudioParams { sample_rate: 48_000, channels: 2 };

fn frames(n: usize) -> Vec<f32> {
    (0..n * 2).map(|i| ((i / 2) as f32 * 0.01).sin() * 0.5).collect()
}

fn close(a: usize, b: usize, tol: usize) -> bool {
    a.abs_diff(b) <= tol
}

#[test]
fn the_ring_takes_a_second_and_the_numbers_do_not_overlap() {
    let mock = MockHost::new();
    let _g = mock.install();
    let mut a = RbAudio::new();
    assert_eq!(a.open(STEREO_48K).unwrap(), STEREO_48K);
    assert!(a.has_device());
    // Three seconds offered, one accepted: a partial write is normal, and the sink never blocks.
    let accepted = a.write(&frames(144_000));
    assert_eq!(accepted, 48_000);
    assert_eq!(a.queued_frames(), 48_000);
    assert_eq!(a.output_latency_us(), 40_000);
    assert_eq!(a.write(&frames(100)), 0, "a full ring takes nothing");
    // The device takes 100 ms: smoothly, to the frame, not in steps of a device period.
    mock.with(|s| s.advance(100_000));
    assert!(close(a.queued_frames(), 48_000 - 4_800, 1), "{}", a.queued_frames());
    mock.with(|s| s.advance(1_000));
    assert!(close(a.queued_frames(), 48_000 - 4_848, 1), "{}", a.queued_frames());
    // What was heard is what the device took minus its own delay (40 ms = 1,920 frames).
    let c = a.clock().expect("audio_clock");
    assert_eq!(c.struct_size, 32);
    assert!(close(c.frames_played as usize, 4_848 - 1_920, 1), "{}", c.frames_played);
    assert_eq!(c.latency_us, 40_000);
    assert_eq!(c.underruns, 0);
    // Paused: the queue is frozen and nothing is lost.
    a.set_paused(true);
    let q = a.queued_frames();
    mock.with(|s| s.advance(500_000));
    assert_eq!(a.queued_frames(), q);
    a.set_paused(false);
    // A flush empties the queue and resets the counters.
    a.flush();
    assert_eq!(a.queued_frames(), 0);
    assert_eq!(a.clock().unwrap().frames_played, 0);
    assert_eq!(mock.with(|s| s.audio.values().next().unwrap().flushes), 1);
    // Samples reach the device unchanged; volume is per stream and clamped.
    a.write(&frames(10));
    a.set_volume(7.0);
    mock.with(|s| {
        let st = s.audio.values().next().unwrap();
        assert_eq!(st.volume, 1.0);
        assert_eq!(&st.samples[st.samples.len() - 20..], &frames(10)[..]);
    });
    a.set_volume(0.25);
    assert_eq!(mock.with(|s| s.audio.values().next().unwrap().volume), 0.25);
}

#[test]
fn without_audio_queued_and_latency_the_same_numbers_come_from_the_clock() {
    let mock = MockHost::new();
    let _g = mock.install();
    mock.with(|s| {
        s.unsupported.insert("audio_queued");
        s.unsupported.insert("audio_latency_us");
    });
    let mut a = RbAudio::new();
    a.open(STEREO_48K).unwrap();
    a.write(&frames(24_000));
    // 100 ms taken by the device, of which the last 40 ms are in its own delay.
    mock.with(|s| s.advance(100_000));
    assert_eq!(a.output_latency_us(), 40_000);
    let q = a.queued_frames();
    assert!(close(q, 24_000 - 4_800, 2), "{q}");
    // A pause freezes it here too.
    a.set_paused(true);
    mock.with(|s| s.advance(200_000));
    assert!(close(a.queued_frames(), q, 2));
}

#[test]
fn opening_again_reuses_the_stream_for_the_same_format_and_follows_a_new_one() {
    let mock = MockHost::new();
    let _g = mock.install();
    let mut a = RbAudio::new();
    a.open(STEREO_48K).unwrap();
    let h = a.stream().unwrap();
    a.write(&frames(1000));
    a.set_paused(true);
    // Next item, same format: the same stream, empty, running (no close and reopen churn).
    assert_eq!(a.open(STEREO_48K).unwrap(), STEREO_48K);
    assert_eq!(a.stream(), Some(h));
    assert_eq!(a.queued_frames(), 0);
    assert_eq!(mock.with(|s| (s.audio.len(), s.audio.values().next().unwrap().paused)), (1, false));
    // A different format: the old stream is closed and a new one opened.
    let mono = AudioParams { sample_rate: 44_100, channels: 1 };
    assert_eq!(a.open(mono).unwrap(), mono);
    assert_ne!(a.stream(), Some(h));
    assert_eq!(mock.with(|s| (s.audio.len(), s.audio_closed.clone())), (1, vec![h]));
    // More than two channels is mixed down by the player; we only ever ask for one or two.
    let six = AudioParams { sample_rate: 48_000, channels: 6 };
    assert_eq!(a.open(six).unwrap().channels, 2);
    // Nonsense is refused.
    assert!(a.open(AudioParams { sample_rate: 0, channels: 2 }).is_err());
}

#[test]
fn the_granted_format_wins() {
    let mock = MockHost::new();
    let _g = mock.install();
    mock.with(|s| s.audio_grant_rate = Some(44_100));
    let mut a = RbAudio::new();
    let got = a.open(STEREO_48K).unwrap();
    assert_eq!(got, AudioParams { sample_rate: 44_100, channels: 2 });
    // The ring is a second at the *granted* rate.
    assert_eq!(a.write(&frames(100_000)), 44_100);
}

#[test]
fn a_device_change_keeps_the_stream_and_an_error_goes_silent_once() {
    let mut h = Harness::new();
    h.play_tone(3.0);
    let stream = h.player.host.audio.stream().unwrap();
    h.push(events::audio_device_changed(stream, 44_100, 2));
    h.run_ms(100);
    assert_eq!(h.player.host.audio.stream(), Some(stream), "the handle survives a device change");
    assert_eq!(h.player.host.audio.native_format(), Some((44_100, 2)));
    assert_eq!(h.mock.with(|s| s.audio_closed.clone()), Vec::<i32>::new());
    // The device is lost: the stream is closed, playback goes on silently on the clock, and the user is told once.
    let pos = h.player.app.model().position_us;
    h.push(events::audio_error(stream, sys::err::NO_DEVICE));
    h.run_ms(500);
    assert!(!h.player.host.audio.has_device());
    assert_eq!(h.mock.with(|s| s.audio_closed.clone()), vec![stream]);
    assert!(h.player.app.model().position_us > pos + 300_000, "the position keeps moving without a device");
    assert!(h.player.host.audio.take_failure().is_none(), "the driver already reported it");
    // An error for some other stream is not ours.
    h.push(events::audio_error(9999, sys::err::IO));
    h.run_ms(50);
}

#[test]
fn no_audio_capability_plays_silently_on_the_clock() {
    let mut h = Harness::with(|s| s.caps = caps::CANVAS | caps::NOW_PLAYING);
    h.open_file(bucket_v0_mock::FileSpec::new("tone.wav", common::tone_wav(2.0)));
    h.run_ms(1000);
    assert!(h.mock.with(|s| s.audio.is_empty()));
    let m = h.player.app.model().clone();
    assert!(m.state.is_active(), "{:?}", m.state);
    assert!(m.position_us > 600_000 && m.position_us < 1_100_000, "{}", m.position_us);
}

#[test]
fn a_file_plays_through_the_whole_chain_and_the_position_follows_what_is_heard() {
    let mut h = Harness::new();
    h.play_tone(2.0);
    h.run_ms(1000);
    let (rate, ch, written, queued) = h.mock.with(|s| {
        let a = s.audio.values().next().unwrap();
        (a.rate, a.channels, a.written, a.written - a.consumed as u64)
    });
    assert_eq!((rate, ch), (48_000, 2));
    assert!(written > 48_000, "the app feeds the ring ahead of the device: {written}");
    assert!(queued <= 48_000);
    // The position is what was heard: about a second minus the device's 40 ms delay, never ahead of the device.
    let pos = h.player.app.model().position_us;
    assert!((800_000..=1_000_000).contains(&pos), "{pos}");
    // The tone arrived: first non-silent samples are the sine (the same value on both channels).
    h.mock.with(|s| {
        let st = s.audio.values().next().unwrap();
        assert!(st.samples.iter().take(2000).any(|v| v.abs() > 0.1));
        assert!(st.samples.chunks(2).take(2000).all(|c| c[0] == c[1]));
    });
    // The app kept up: no underrun so far.
    assert_eq!(h.mock.with(|s| s.audio.values().next().map_or(0, |a| a.underruns)), 0);
    // And the file plays to its end.
    assert!(h.run_until(3000, |h| h.player.app.model().state == rvp_ui::MediaState::Ended));
    let _ = ev::WAKE;
}
