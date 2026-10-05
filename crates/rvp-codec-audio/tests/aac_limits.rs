//! What the AAC decoder (symphonia 0.6) cannot do is reported as `Unsupported` at build time, so a player can say so and go on
//! without sound: HE-AAC (SBR, with or without PS) and AAC with more than two channels. AAC-LC stereo and mono are fine.
use rvp_codec_audio::audio_decoder;
use rvp_core::{AudioInfo, Error, Rational, StreamInfo, StreamKind};

fn aac(asc: &[u8], channels: u16) -> StreamInfo {
    StreamInfo {
        id: 1,
        kind: StreamKind::Audio,
        codec: "aac".into(),
        time_base: Rational::new(1, 1_000_000),
        language: None,
        extra_data: asc.to_vec(),
        video: None,
        audio: Some(AudioInfo { sample_rate: 48_000, channels }),
        duration_us: None,
    }
}

/// An AudioSpecificConfig from fields: object type, sampling frequency index, channel configuration, and optionally the
/// backward compatible SBR signalling (sync extension 0x2b7, extension object type 5, flag, extension frequency index).
fn asc(aot: u32, freq: u32, chan: u32, sbr: Option<u32>) -> Vec<u8> {
    let mut bits = String::new();
    let mut put = |v: u32, n: usize| bits.push_str(&format!("{v:0n$b}"));
    put(aot, 5);
    put(freq, 4);
    put(chan, 4);
    if let Some(ext_freq) = sbr {
        put(0x2b7, 11);
        put(5, 5);
        put(1, 1);
        put(ext_freq, 4);
    }
    while bits.len() % 8 != 0 {
        bits.push('0');
    }
    bits.as_bytes()
        .chunks(8)
        .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 2).unwrap())
        .collect()
}

#[test]
fn lc_mono_and_stereo_build() {
    assert_eq!(asc(2, 3, 2, None), [0x11, 0x90]);
    assert!(audio_decoder(&aac(&asc(2, 3, 2, None), 2)).is_ok());
    assert!(audio_decoder(&aac(&asc(2, 3, 1, None), 1)).is_ok());
}

#[test]
fn he_aac_plays_as_its_core_or_is_unsupported_and_multichannel_aac_is_unsupported() {
    // HE-AAC with backward compatible signalling (a 24 kHz LC core, SBR to 48 kHz announced after it): symphonia skips the SBR
    // data, so what is decoded is the core at its own sample rate, a band-limited version of the sound.
    let he = audio_decoder(&aac(&asc(2, 6, 2, Some(3)), 2));
    assert!(he.is_ok(), "HE-AAC core: {:?}", he.err());
    // Explicit hierarchical signalling (object type 5, SBR, in front) is refused.
    let sbr_first = [0b0010_1011, 0b0001_0001, 0b1000_1000, 0b0000_0000];
    let r = audio_decoder(&aac(&sbr_first, 2));
    assert!(matches!(r, Err(Error::Unsupported(_))), "object type 5: {:?}", r.err());
    // 5.1 (channel configuration 6), 7.1 (7) and 3.0.
    for (cfg, ch) in [(6, 6), (7, 8), (3, 3)] {
        let r = audio_decoder(&aac(&asc(2, 3, cfg, None), ch));
        assert!(matches!(r, Err(Error::Unsupported(_))), "{ch} channels: {:?}", r.err());
    }
}
