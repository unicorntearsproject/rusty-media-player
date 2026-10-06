//! The shell's media interface: now-playing and transport, the visualizer feed, and the sleep inhibitors.
mod common;
use bucket_v0_mock::{MockHost, events};
use bucket_v0_sys::{self as sys, caps, ev, play_state, playback_flags, power, transport};
use common::Harness;
use rvp_core::Art;
use rvp_host::{NowPlaying, NowPlayingMeta, PlayState, Playback, TransportCommand};
use rvp_host_rb::media::{RbNowPlaying, transport_command, truncate_utf8};
use rvp_host_rb::shared::Limits;

/// The OS changes what it offers: its state and the event.
fn set_caps(h: &mut Harness, caps: i64) {
    h.mock.with(|s| s.caps = caps);
    h.push(events::caps_changed(caps));
}

fn limits() -> Limits {
    Limits { art: 1000, string: 20, read: 1 << 20, events: 256, threads: 64 }
}

#[test]
fn a_playing_file_is_reported_with_its_state_and_a_clock_stamp() {
    let mut h = Harness::new();
    h.play_tone(10.0);
    h.run_ms(300);
    let (meta, pb) = h.mock.with(|s| (s.metadata.clone(), s.playback.clone()));
    assert!(!meta.is_empty());
    let m = meta.last().unwrap();
    assert_eq!(m.struct_size, 64);
    assert!(m.title.contains("tone"), "{}", m.title);
    assert!(!m.has_video && m.art.is_empty());
    assert!((9_900_000..=10_100_000).contains(&m.duration_us), "{}", m.duration_us);
    // Playback: playing, seekable, rate 1, stamped with the OS clock (extrapolation needs the time the position was true).
    let p = pb.iter().rev().find(|p| p.state == play_state::PLAYING).expect("a playing report");
    assert_eq!((p.struct_size, p.rate), (40, 1.0));
    assert_ne!(p.flags & playback_flags::CAN_SEEK, 0);
    assert_eq!(
        p.flags & !(playback_flags::CAN_NEXT | playback_flags::CAN_PREV | playback_flags::CAN_SEEK),
        0,
        "only defined bits"
    );
    assert!(p.host_time_us >= 1_000_000 && p.host_time_us <= h.now(), "{} {}", p.host_time_us, h.now());
    // Not every tick: the state did not change, so the shell extrapolates and nothing more is sent.
    let n = h.mock.with(|s| s.playback.len());
    h.run_ms(1000);
    assert_eq!(h.mock.with(|s| s.playback.len()), n);
}

#[test]
fn transport_commands_from_the_shell_drive_the_player() {
    let mut h = Harness::new();
    h.play_tone(30.0);
    h.run_ms(300);
    let state = |h: &Harness| h.player.app.model().state;
    h.push(events::transport(transport::PAUSE, 0));
    h.run_ms(100);
    assert_eq!(state(&h), rvp_ui::MediaState::Paused);
    assert_eq!(h.mock.with(|s| s.playback.last().unwrap().state), play_state::PAUSED);
    // A command that does not apply is ignored (Pause while paused), Toggle resumes.
    h.push(events::transport(transport::PAUSE, 0));
    h.run_ms(50);
    assert_eq!(state(&h), rvp_ui::MediaState::Paused);
    h.push(events::transport(transport::TOGGLE, 0));
    h.run_ms(100);
    assert_eq!(state(&h), rvp_ui::MediaState::Playing);
    // Seeks are microseconds: absolute, then relative.
    h.push(events::transport(transport::SEEK_TO, 20_000_000));
    h.run_ms(200);
    let p = h.player.app.model().position_us;
    assert!((19_900_000..=20_500_000).contains(&p), "{p}");
    h.push(events::transport(transport::SEEK_BY, -5_000_000));
    h.run_ms(200);
    let p = h.player.app.model().position_us;
    assert!((14_900_000..=15_500_000).contains(&p), "{p}");
    // The report after a jump carries the new position.
    let last = h.mock.with(|s| *s.playback.last().unwrap());
    assert!((14_800_000..=15_500_000).contains(&last.position_us), "{}", last.position_us);
    // Volume reaches the stream; an unknown command and Next without a next item do nothing.
    h.push(events::transport_f32(transport::SET_VOLUME, 0.5));
    h.push(events::transport(77, 0));
    h.push(events::transport(transport::NEXT, 0));
    h.run_ms(100);
    assert_eq!(state(&h), rvp_ui::MediaState::Playing);
    assert_eq!(h.mock.with(|s| s.audio.values().next().unwrap().volume), 0.5);
    // Stop pauses and goes back to the start.
    h.push(events::transport(transport::STOP, 0));
    h.run_ms(200);
    assert_ne!(state(&h), rvp_ui::MediaState::Playing);
    assert!(h.player.app.model().position_us < 500_000);
}

#[test]
fn transport_values_decode() {
    assert_eq!(transport_command(transport::SEEK_BY, -3, 0.0), Some(TransportCommand::SeekBy(-3)));
    assert_eq!(transport_command(transport::SET_RATE, 0, 1.5), Some(TransportCommand::SetRate(1.5)));
    assert_eq!(transport_command(transport::SET_RATE, 0, 0.0), None);
    assert_eq!(transport_command(transport::SET_RATE, 0, f32::NAN), None);
    assert_eq!(transport_command(transport::SET_VOLUME, 0, 3.0), Some(TransportCommand::SetVolume(1.0)));
    assert_eq!(transport_command(99, 0, 0.0), None);
    // The f32 sits in the low four bytes of the value, as the events page says.
    let e = events::transport_f32(transport::SET_VOLUME, 0.25);
    assert_eq!(e.f32_at(24), 0.25);
}

#[test]
fn text_is_cut_at_a_character_and_covers_the_shell_cannot_take_are_left_out() {
    assert_eq!(truncate_utf8("héllo", 2), "h");
    assert_eq!(truncate_utf8("héllo", 3), "hé");
    assert_eq!(truncate_utf8("日本語", 4), "日");
    assert_eq!(truncate_utf8("abc", 10), "abc");
    assert_eq!(truncate_utf8("日本語", 0), "");

    let mock = MockHost::new();
    let _g = mock.install();
    mock.with(|s| s.limits.insert(sys::limit::STRING_BYTES, 20));
    let mut np = RbNowPlaying::new(limits());
    // The OS takes a cover only if its bytes start like a PNG or a JPEG.
    let png_of = |n: usize| {
        let mut d = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        d.resize(n, 7);
        d
    };
    let png = Art { mime: "image/png".into(), data: png_of(500) };
    let meta = NowPlayingMeta {
        title: "日本語日本語日本語日本語".into(),
        artist: "A".into(),
        album: String::new(),
        art: Some(png.clone()),
        duration_us: None,
        has_video: true,
    };
    np.set_metadata(&meta);
    let m = mock.with(|s| s.metadata.last().cloned()).unwrap();
    assert_eq!(m.title, "日本語日本語", "20 bytes cut at a character boundary (18)");
    assert_eq!((m.artist.as_str(), m.album.as_str(), m.art_mime.as_str()), ("A", "", "image/png"));
    assert_eq!(m.art.len(), 500);
    assert_eq!(m.duration_us, -1, "unknown duration is -1");
    assert!(m.has_video);
    // Over the art limit, or not PNG/JPEG: the cover is dropped and the rest is still reported.
    np.set_metadata(&NowPlayingMeta {
        art: Some(Art { mime: "image/png".into(), data: png_of(5000) }),
        ..meta.clone()
    });
    np.set_metadata(&NowPlayingMeta {
        art: Some(Art { mime: "image/gif".into(), data: vec![1; 50] }),
        ..meta.clone()
    });
    let all = mock.with(|s| s.metadata.clone());
    assert_eq!(all.len(), 3);
    assert!(all[1].art.is_empty() && all[2].art.is_empty());
    assert_eq!(np.art_dropped, 2);
    // The OS's limit is lower than the one we read: it says TOO_LARGE and the report is repeated without the cover.
    mock.with(|s| s.limits.insert(sys::limit::ART_BYTES, 100));
    np.set_metadata(&meta);
    let all = mock.with(|s| s.metadata.clone());
    assert_eq!(all.len(), 4);
    assert!(all[3].art.is_empty());
    assert_eq!(all[3].title, all[0].title);
    // A picture whose bytes are not a PNG or a JPEG is `-INVALID` for the OS: it is left out before the call.
    np.set_metadata(&NowPlayingMeta {
        art: Some(Art { mime: "image/png".into(), data: vec![1; 50] }),
        ..meta.clone()
    });
    let all = mock.with(|s| s.metadata.clone());
    assert_eq!(all.len(), 5);
    assert!(all[4].art.is_empty());
    assert_eq!(mock.with(|s| s.truncated_strings), 0, "the strings were cut by us, not by the OS");
}

#[test]
fn reports_the_os_would_refuse_are_made_valid() {
    let mock = MockHost::new();
    let _g = mock.install();
    let mut np = RbNowPlaying::new(limits());
    let base = Playback {
        state: PlayState::Playing,
        position_us: 5,
        rate: 1.0,
        can_next: false,
        can_prev: false,
        can_seek: true,
    };
    // A negative position and a rate that is not a number are `-INVALID`; the adapter sends 0 and 1 instead.
    np.set_playback(&Playback { position_us: -40, rate: f32::NAN, ..base });
    let p = mock.with(|s| s.playback.clone());
    assert_eq!(p.len(), 1, "the mock accepted the report");
    assert_eq!((p[0].position_us, p[0].rate), (0, 1.0));
    assert!(mock.with(|s| s.logs.iter().all(|(l, _)| *l > 1)));
}

#[test]
fn playback_flags_and_states_map() {
    let mock = MockHost::new();
    let _g = mock.install();
    let mut np = RbNowPlaying::new(limits());
    let base = Playback {
        state: PlayState::Paused,
        position_us: 123,
        rate: 1.5,
        can_next: true,
        can_prev: false,
        can_seek: true,
    };
    np.set_playback(&base);
    np.set_playback(&Playback {
        state: PlayState::Stopped,
        can_next: false,
        can_prev: true,
        can_seek: false,
        ..base
    });
    let pb = mock.with(|s| s.playback.clone());
    assert_eq!(pb[0].state, play_state::PAUSED);
    assert_eq!(pb[0].flags, playback_flags::CAN_NEXT | playback_flags::CAN_SEEK);
    assert_eq!((pb[0].position_us, pb[0].rate), (123, 1.5));
    assert_eq!(pb[1].state, play_state::STOPPED);
    assert_eq!(pb[1].flags, playback_flags::CAN_PREV);
    assert_eq!((pb[0].reserved0, pb[0].reserved1), (0, 0));
}

#[test]
fn the_shell_that_starts_late_is_told_what_plays_and_the_session_is_given_up_at_the_end() {
    let mut h = Harness::with(|s| s.caps = caps::CANVAS | caps::AUDIO_OUT);
    h.play_tone(10.0);
    h.run_ms(200);
    assert!(h.mock.with(|s| s.metadata.is_empty() && s.playback.is_empty()), "no shell, nothing sent");
    set_caps(&mut h, caps::CANVAS | caps::AUDIO_OUT | caps::NOW_PLAYING);
    h.run_ms(100);
    let (m, p) = h.mock.with(|s| (s.metadata.clone(), s.playback.clone()));
    assert_eq!(m.len(), 1, "the item that is playing is reported once");
    assert_eq!(p.last().map(|p| p.state), Some(play_state::PLAYING));
    // The shell hides and shows now-playing again: it lost what it knew, so it is told again (the app does not resend what it
    // thinks the shell has).
    set_caps(&mut h, caps::CANVAS | caps::AUDIO_OUT);
    h.run_ms(50);
    set_caps(&mut h, caps::CANVAS | caps::AUDIO_OUT | caps::NOW_PLAYING);
    h.run_ms(100);
    assert_eq!(h.mock.with(|s| s.metadata.len()), 2);
    assert_eq!(h.mock.with(|s| s.playback.last().map(|p| p.state)), Some(play_state::PLAYING));
    // The app ends: it gives the session up.
    h.push(events::with_u32(ev::TERMINATE, 1000));
    assert!(!h.player.run_once());
    assert_eq!(h.mock.with(|s| s.cleared), 1);
}

#[test]
fn the_machine_stays_awake_while_audio_plays_and_the_display_only_for_pictures() {
    let mut h = Harness::new();
    h.run_ms(100);
    assert!(h.mock.with(|s| s.power.is_empty()), "idle: nothing is held");
    h.play_tone(30.0);
    h.run_ms(200);
    assert_eq!(
        h.mock.with(|s| s.power.clone()),
        vec![(power::SYSTEM, true)],
        "audio keeps the system awake, not the screen"
    );
    h.push(events::transport(transport::PAUSE, 0));
    h.run_ms(100);
    assert_eq!(h.mock.with(|s| s.power.last().copied()), Some((power::SYSTEM, false)));
    h.push(events::transport(transport::PLAY, 0));
    h.run_ms(100);
    assert_eq!(h.mock.with(|s| s.power.last().copied()), Some((power::SYSTEM, true)));
    // Ending releases it (the OS also does when the app goes).
    h.push(events::with_u32(ev::TERMINATE, 1000));
    assert!(!h.player.run_once());
    assert_eq!(h.mock.with(|s| s.power.last().copied()), Some((power::SYSTEM, false)));
}

#[test]
fn the_visualizer_feed_is_batched_and_only_runs_while_the_shell_wants_it() {
    let mut h = Harness::with(|s| s.caps |= caps::VISUALIZER);
    h.play_tone(10.0);
    h.run_ms(2000);
    let (blocks, sums, batches, singles) = h
        .mock
        .with(|s| (s.viz_blocks.clone(), s.viz_summaries.clone(), s.viz_batch_calls, s.viz_single_calls));
    assert!(!blocks.is_empty());
    assert_eq!(singles, 0);
    // About 94 summaries a second (a hop of 512 frames at 48 kHz) of what was heard, in far fewer calls.
    let secs = h.player.app.model().position_us as f64 / 1e6;
    let per_sec = sums.len() as f64 / secs;
    assert!((70.0..=110.0).contains(&per_sec), "{per_sec} summaries per second over {secs} s");
    assert!(batches as usize <= sums.len() && batches > 0, "{batches} calls for {} summaries", sums.len());
    for s in &sums {
        assert_eq!(s.struct_size, 176);
        assert!((0.0..=1.0).contains(&s.level) && (0.0..=1.0).contains(&s.peak));
        assert!(s.bands.iter().all(|b| (0.0..=1.0).contains(b)));
    }
    assert!(sums.windows(2).all(|w| w[1].pts_us >= w[0].pts_us));
    // 440 Hz: the loudest band is a low one, and the tone is loud.
    let loud = sums.iter().rev().find(|s| s.level > 0.1).expect("a loud summary");
    let top = loud.bands.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
    assert!(top < 16, "band {top}");
    // PCM blocks come stamped with the stream time, the rate and the channel count.
    assert!(blocks.iter().all(|b| b.1 == 48_000 && b.2 == 2 && b.3 > 0));
    // The shell hides the visualizer: the feed stops (the analysis is not even computed).
    set_caps(&mut h, caps::CANVAS | caps::AUDIO_OUT | caps::NOW_PLAYING | caps::LIBRARY);
    h.run_ms(50);
    let n = h.mock.with(|s| (s.viz_blocks.len(), s.viz_summaries.len()));
    h.run_ms(500);
    assert_eq!(h.mock.with(|s| (s.viz_blocks.len(), s.viz_summaries.len())), n);
}

#[test]
fn the_summaries_of_one_tick_go_in_one_call() {
    use rvp_host::{VisualizerTap, VizSummary};
    let mock = MockHost::new();
    let _g = mock.install();
    let mut viz = rvp_host_rb::media::RbViz::new();
    let one = |pts: i64| VizSummary {
        pts_us: pts,
        level: 0.5,
        peak: 0.9,
        bands: std::array::from_fn(|i| i as f32 / 32.0),
        bass: 0.1,
        mid: 0.2,
        treble: 0.3,
        onset: true,
        onset_strength: 2.5,
        tempo_bpm: 120.0,
    };
    viz.push_summary(&one(0));
    viz.push_summary(&one(10_667));
    viz.push_summary(&one(21_333));
    assert_eq!(mock.with(|s| s.viz_batch_calls), 0, "nothing is sent before the tick ends");
    viz.flush();
    viz.flush();
    let (calls, sums) = mock.with(|s| (s.viz_batch_calls, s.viz_summaries.clone()));
    assert_eq!((calls, sums.len()), (1, 3));
    // Every field lands in the API's layout.
    let s = &sums[2];
    assert_eq!((s.struct_size, s.onset, s.pts_us), (176, 1, 21_333));
    assert_eq!(
        (s.level, s.peak, s.bass, s.mid, s.treble, s.onset_strength, s.tempo_bpm),
        (0.5, 0.9, 0.1, 0.2, 0.3, 2.5, 120.0)
    );
    assert_eq!(s.bands[31], 31.0 / 32.0);
    // A long stretch without a tick end is flushed at 64 so memory stays small.
    for i in 0..130 {
        viz.push_summary(&one(i));
    }
    assert_eq!(mock.with(|s| s.viz_batch_calls), 3);
}

#[test]
fn without_viz_summary_n_the_summaries_go_one_by_one() {
    let mut h = Harness::with(|s| {
        s.caps |= caps::VISUALIZER;
        s.unsupported.insert("viz_summary_n");
    });
    h.play_tone(5.0);
    h.run_ms(1000);
    let (sums, batches, singles) =
        h.mock.with(|s| (s.viz_summaries.len(), s.viz_batch_calls, s.viz_single_calls));
    assert!(sums > 50);
    assert_eq!((batches, singles as usize), (0, sums), "nothing is lost when the batch call is missing");
}
