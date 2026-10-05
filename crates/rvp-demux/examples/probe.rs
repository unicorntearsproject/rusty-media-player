//! Print the streams and packet count of a file: `cargo run -p rvp-demux --example probe -- file`.
use rvp_core::task::block_on;
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;

fn main() {
    let path = std::env::args().nth(1).expect("usage: probe <file>");
    block_on(async {
        let mut d = open(MemSource::new(std::fs::read(&path).unwrap())).await.unwrap();
        println!("container duration: {:?}", d.duration_us());
        for s in d.streams() {
            println!(
                "{} {:?} {} dur={:?} lang={:?} audio={:?} video={:?} extra={}",
                s.id,
                s.kind,
                s.codec,
                s.duration_us,
                s.language,
                s.audio,
                s.video,
                s.extra_data.len()
            );
        }
        let (mut n, mut last) = (0, None);
        while let Some(p) = d.next_packet().await.unwrap() {
            n += 1;
            last = Some((p.stream_id, p.pts, p.duration));
        }
        println!("{n} packets, last {last:?}");
    });
}
