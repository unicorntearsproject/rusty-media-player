//! Files that are still being written: a demuxer opened on a prefix of a file reads what is there, and once the source
//! has grown it carries on with exactly the packets a complete read gives.
use rvp_core::task::block_on;
use rvp_core::{Packet, Result};
use rvp_demux::{Demuxer, open};
use rvp_host::{HostError, Source};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

fn fixtures() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root.join("target/fixtures"))
}

/// A source whose length can be raised from outside.
struct Growing {
    data: Rc<Vec<u8>>,
    len: Rc<RefCell<usize>>,
}

impl Source for Growing {
    async fn size(&self) -> Option<u64> {
        Some(*self.len.borrow() as u64)
    }
    async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> std::result::Result<usize, HostError> {
        let len = *self.len.borrow();
        let off = offset as usize;
        if off >= len {
            return Ok(0);
        }
        let n = buf.len().min(len - off);
        buf[..n].copy_from_slice(&self.data[off..off + n]);
        Ok(n)
    }
    fn name(&self) -> &str {
        "growing"
    }
}

fn drain<D: Demuxer>(d: &mut D) -> Vec<Packet> {
    let mut v = Vec::new();
    // Cut off in the middle of a packet or at its end: this is where the file ends for now.
    while let Ok(Some(p)) = block_on(d.next_packet()) {
        v.push(p);
    }
    v
}

fn run(name: &str, first_pct: usize) -> Result<()> {
    let Ok(full) = std::fs::read(fixtures().join(name)) else { return Ok(()) };
    let data = Rc::new(full);
    // The reference: every packet of the complete file.
    let whole = Growing { data: data.clone(), len: Rc::new(RefCell::new(data.len())) };
    let mut d = block_on(open(whole))?;
    let want = drain(&mut d);
    // The same file, opened when only a part of it was there and read on after it grew.
    let len = Rc::new(RefCell::new(data.len() * first_pct / 100));
    let src = Growing { data: data.clone(), len: len.clone() };
    let mut d = block_on(open(src))?;
    let mut got = drain(&mut d);
    assert!(got.len() < want.len(), "{name}: a {first_pct}% prefix already gave every packet");
    *len.borrow_mut() = data.len() * (first_pct + 25).min(100) / 100;
    got.extend(drain(&mut d));
    *len.borrow_mut() = data.len();
    got.extend(drain(&mut d));
    assert_eq!(got.len(), want.len(), "{name}: packet count after growth");
    for (i, (a, b)) in got.iter().zip(&want).enumerate() {
        assert!(a == b, "{name}: packet {i} differs");
    }
    Ok(())
}

#[test]
fn fragmented_mp4_grows() {
    run("h264_aac_frag.mp4", 40).unwrap();
}

#[test]
fn faststart_mp4_grows() {
    run("h264_aac_faststart.mp4", 40).unwrap();
}

#[test]
fn matroska_grows() {
    run("h264_flac.mkv", 40).unwrap();
}

#[test]
fn webm_grows() {
    run("vp9_vorbis.webm", 40).unwrap();
}
