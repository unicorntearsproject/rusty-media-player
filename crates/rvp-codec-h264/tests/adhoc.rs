//! Debug helper: compare any MP4/MKV with ffmpeg. `RVP_H264_FILE=path cargo test -p rvp-codec-h264 --test adhoc -- --ignored`.
mod common;
use std::path::PathBuf;

#[test]
#[ignore]
fn adhoc() {
    let path = PathBuf::from(std::env::var("RVP_H264_FILE").expect("set RVP_H264_FILE"));
    let d = common::decode_container(&path);
    let want = common::reference(&path);
    match common::compare("adhoc", &d.frames, &want) {
        Ok(()) => println!("OK: {} frames bit-exact, {} errors", d.frames.len(), d.errors),
        Err(e) => panic!("{e} ({} packets, {} errors)", d.packets, d.errors),
    }
}

#[test]
#[ignore]
fn adhoc_pocs() {
    let path = PathBuf::from(std::env::var("RVP_H264_FILE").expect("set RVP_H264_FILE"));
    let d = common::decode_container(&path);
    let pocs: Vec<(i32, i64)> = d.frames.iter().map(|f| (f.poc, f.pts)).collect();
    println!("{pocs:?}");
}

#[test]
#[ignore]
fn adhoc_trace() {
    use rvp_core::StreamKind;
    use rvp_core::task::block_on;
    use rvp_demux::{Demuxer, open};
    use rvp_host::mock::MemSource;
    let path = PathBuf::from(std::env::var("RVP_H264_FILE").expect("set RVP_H264_FILE"));
    let data = std::fs::read(&path).unwrap();
    block_on(async {
        let mut d = open(MemSource::new(data)).await.unwrap();
        let info = d.streams().iter().find(|s| s.kind == StreamKind::Video).unwrap().clone();
        let mut dec = rvp_codec_h264::decoder::Decoder::new();
        dec.set_avcc(&info.extra_data).unwrap();
        let mut n = 0;
        while let Some(p) = d.next_packet().await.unwrap() {
            if p.stream_id != info.id {
                continue;
            }
            dec.decode_sample(&p.data, p.pts).unwrap();
            let mut out = vec![];
            while let Some(f) = dec.next_frame() {
                out.push(f.poc);
            }
            println!("pkt {n}: out {out:?} dpb {:?}", dec.dpb_snapshot());
            n += 1;
            if n > 14 {
                break;
            }
        }
    });
}
