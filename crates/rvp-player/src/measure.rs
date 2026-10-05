//! Measuring the loudness of a whole file: what the library scan does for tracks whose tags do not say.
//!
//! The file is demuxed and decoded the way playback does it, mixed to stereo the way playback does it (so the figure is the
//! loudness of what comes out of the speakers, a mono file counting as the same sound on both) and fed to the BS.1770 meter of
//! `rvp-core`. It works through the cooperative executor: the loop gives the executor a turn every few dozen packets.
use crate::audio::mix;
use alloc::string::ToString;
use rvp_core::task::yield_now;
use rvp_core::{CodecFactory, Error, LoudnessMeter, Measurement, StreamKind};
use rvp_host::Source;

/// Packets decoded between two turns given to the rest of the application.
const YIELD_EVERY: usize = 24;

/// Decode all of the first audio stream of `source` and measure its integrated loudness. `Ok(None)` if there is nothing audible
/// in it (silence, or less than a gate block of sound).
pub async fn measure_source<S: Source>(
    source: S,
    codecs: &dyn CodecFactory,
) -> Result<Option<Measurement>, Error> {
    measure_source_with(source, codecs, false).await
}

/// [`measure_source`], also finding the true (inter-sample) peak when `true_peak` is set (about three times the work).
pub async fn measure_source_with<S: Source>(
    source: S,
    codecs: &dyn CodecFactory,
    true_peak: bool,
) -> Result<Option<Measurement>, Error> {
    let mut demux = rvp_demux::open_quick(source).await?;
    use rvp_demux::Demuxer;
    let info = demux
        .streams()
        .iter()
        .find(|s| s.kind == StreamKind::Audio)
        .ok_or_else(|| Error::Unsupported("no audio stream".to_string()))?
        .clone();
    let mut dec = codecs.audio(&info)?;
    let mut meter: Option<LoudnessMeter> = None;
    let mut packets = 0usize;
    while let Some(p) = demux.next_packet().await? {
        if p.stream_id != info.id {
            continue;
        }
        packets += 1;
        if packets % YIELD_EVERY == 0 {
            yield_now().await;
        }
        // A packet that does not decode is skipped, as in playback.
        if dec.send_packet(&p).is_err() {
            continue;
        }
        while let Ok(Some(buf)) = dec.receive_buffer() {
            // The priming before time zero is not part of the programme.
            if buf.pts + buf.duration_us() <= 0 {
                continue;
            }
            let m = meter.get_or_insert_with(|| {
                let m = LoudnessMeter::new(buf.params.sample_rate.max(8000), 2);
                if true_peak { m.with_true_peak() } else { m }
            });
            if buf.params.sample_rate == m.sample_rate() {
                m.process(&mix(&buf.samples, buf.params.channels.max(1) as usize, 2));
            }
        }
    }
    Ok(meter.and_then(|m| m.measurement()))
}
