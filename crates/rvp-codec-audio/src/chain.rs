//! Chained Ogg: a decoder that rebuilds itself when a new logical stream's headers arrive in the packet flow.
//!
//! The Ogg demuxer hands the headers of every link after the first to the decoder as ordinary packets (`rvp-demux`,
//! `ogg.rs`): the three Vorbis headers (types 1, 3 and 5, which cannot be audio because the low bit of an audio packet's
//! first byte is zero), an `OpusHead`, a FLAC mapping packet (`0x7f FLAC ...`). This wrapper recognises them, builds a new
//! decoder from them, and for Opus drops the pre-skip from the start of the link (at the start of a file the player does that
//! itself with the negative first timestamp; in the middle of a chain there is no such thing). Packets of any other kind pass
//! straight through, so a Matroska or MP4 stream, which never has such packets, is unaffected.
use crate::{opus, sym};
use rvp_core::{AudioBuffer, AudioDecoder, AudioInfo, Error, Packet, Result, StreamInfo};

pub(crate) struct ChainDec {
    inner: Box<dyn AudioDecoder>,
    info: StreamInfo,
    /// Vorbis header packets collected so far.
    hdrs: Vec<Vec<u8>>,
    /// Opus: samples (at 48 kHz) still to drop from the start of the decoder's output.
    skip: i64,
}

impl ChainDec {
    pub(crate) fn new(info: &StreamInfo, inner: Box<dyn AudioDecoder>) -> Self {
        Self { inner, info: info.clone(), hdrs: Vec::new(), skip: 0 }
    }

    fn rebuild(&mut self) -> Result<()> {
        self.inner = match self.info.codec.as_str() {
            "opus" => Box::new(opus::OpusDec::new(&self.info)?),
            _ => Box::new(sym::SymphoniaDec::new(&self.info)?),
        };
        Ok(())
    }

    /// If `p` is a header packet of a new link, take it in. `true` when it was one.
    fn header(&mut self, p: &Packet) -> Result<bool> {
        let d = &p.data;
        match self.info.codec.as_str() {
            "vorbis" if d.len() >= 7 && d[0] & 1 == 1 && &d[1..7] == b"vorbis" => {
                if d[0] == 1 {
                    self.hdrs.clear();
                }
                self.hdrs.push(d.clone());
                if d[0] == 5 && self.hdrs.len() >= 3 {
                    let id = &self.hdrs[0];
                    if id.len() < 30 {
                        return Err(Error::Invalid("short Vorbis identification header".into()));
                    }
                    // Xiph lacing of the three headers, as Matroska stores them.
                    let mut x = vec![2u8];
                    for n in [self.hdrs[0].len(), self.hdrs[1].len()] {
                        x.extend(std::iter::repeat_n(255u8, n / 255));
                        x.push((n % 255) as u8);
                    }
                    for h in &self.hdrs {
                        x.extend_from_slice(h);
                    }
                    self.info.audio = Some(AudioInfo {
                        sample_rate: u32::from_le_bytes([id[12], id[13], id[14], id[15]]),
                        channels: id[11] as u16,
                    });
                    self.info.extra_data = x;
                    self.hdrs.clear();
                    self.rebuild()?;
                }
                Ok(true)
            }
            "opus" if d.starts_with(b"OpusHead") && d.len() >= 19 => {
                self.info.audio = Some(AudioInfo { sample_rate: 48_000, channels: d[9] as u16 });
                self.info.extra_data = d.clone();
                self.rebuild()?;
                // The demuxer puts the pre-skip to be dropped (microseconds) in the packet's duration.
                self.skip = p.duration.max(0) * 48 / 1000;
                Ok(true)
            }
            "flac" if d.len() >= 9 + 42 && d.starts_with(b"\x7fFLAC") => {
                let si = &d[9..9 + 42]; // "fLaC", block header, STREAMINFO
                let packed =
                    u64::from_be_bytes([si[18], si[19], si[20], si[21], si[22], si[23], si[24], si[25]]);
                self.info.audio = Some(AudioInfo {
                    sample_rate: (packed >> 44) as u32,
                    channels: (((packed >> 41) & 7) + 1) as u16,
                });
                self.info.extra_data = si.to_vec();
                self.rebuild()?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

impl AudioDecoder for ChainDec {
    fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        if self.header(packet)? {
            return Ok(());
        }
        self.inner.send_packet(packet)
    }

    fn receive_buffer(&mut self) -> Result<Option<AudioBuffer>> {
        loop {
            let Some(mut b) = self.inner.receive_buffer()? else { return Ok(None) };
            if self.skip > 0 {
                let ch = b.params.channels.max(1) as usize;
                let frames = (b.samples.len() / ch) as i64;
                let drop = self.skip.min(frames);
                self.skip -= drop;
                b.samples.drain(..drop as usize * ch);
                b.pts += drop * 1_000_000 / b.params.sample_rate.max(1) as i64;
                if b.samples.is_empty() {
                    continue;
                }
            }
            return Ok(Some(b));
        }
    }

    fn flush(&mut self) {
        self.skip = 0;
        self.inner.flush();
    }
}
