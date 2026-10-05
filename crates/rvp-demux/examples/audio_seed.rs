//! Writes a fuzz seed for `fuzz/fuzz_targets/audio.rs` from a media file: `audio_seed <file> <out> [packets]`.
use rvp_core::StreamKind;
use rvp_core::task::block_on;
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;

fn main() {
    let mut a = std::env::args().skip(1);
    let (file, out) = (a.next().expect("file"), a.next().expect("out"));
    let n: usize = a.next().map_or(40, |v| v.parse().unwrap());
    block_on(async {
        let mut d = open(MemSource::new(std::fs::read(file).unwrap())).await.unwrap();
        let info = d.streams().iter().find(|s| s.kind == StreamKind::Audio).unwrap().clone();
        let code =
            ["aac", "mp3", "flac", "vorbis", "opus"].iter().position(|c| *c == info.codec).expect("codec")
                as u8;
        let mut v = vec![code, info.extra_data.len().min(255) as u8];
        v.extend_from_slice(&info.extra_data[..info.extra_data.len().min(255)]);
        let mut count = 0;
        while let Some(p) = d.next_packet().await.unwrap() {
            if p.stream_id == info.id {
                v.extend_from_slice(&(p.data.len().min(65535) as u16).to_le_bytes());
                v.extend_from_slice(&p.data[..p.data.len().min(65535)]);
                count += 1;
                if count >= n {
                    break;
                }
            }
        }
        std::fs::write(out, v).unwrap();
    });
}
