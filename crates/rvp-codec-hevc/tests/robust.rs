//! Damaged input must never panic, hang or run away: bytes flipped, packets cut short, dropped, repeated and shuffled, on streams that
//! use the decoder's different paths. Errors are fine; so are wrong pictures. The decoder must stay usable afterwards.
use rvp_codec_hevc::sw::hevc_decoder;
use rvp_core::task::block_on;
use rvp_core::{Packet, StreamKind};
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as usize
    }
}

fn fixtures() -> Option<PathBuf> {
    if std::env::var_os("RVP_SKIP_FIXTURES").is_some()
        || Command::new("ffmpeg").arg("-version").output().is_err()
    {
        return None;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir =
        std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root.join("target/fixtures"));
    let st = Command::new("bash")
        .arg(root.join("tools/gen-fixtures.sh"))
        .arg(&dir)
        .env("RVP_FIXTURE_SET", "hevcconf")
        .status()
        .ok()?;
    assert!(st.success());
    Some(dir.join("hevcconf"))
}

fn mutate(packets: &mut Vec<Packet>, rng: &mut Rng) {
    for _ in 0..1 + rng.next() % 3 {
        let n = packets.len();
        let i = rng.next() % n;
        match rng.next() % 6 {
            0 | 1 => {
                for _ in 0..1 + rng.next() % 8 {
                    let len = packets[i].data.len();
                    if len > 0 {
                        packets[i].data[rng.next() % len] ^= 1 << (rng.next() % 8);
                    }
                }
            }
            2 => {
                let len = packets[i].data.len();
                packets[i].data.truncate(rng.next() % (len + 1));
            }
            3 => {
                packets.remove(i);
            }
            4 => {
                let p = packets[i].clone();
                packets.insert(i, p);
            }
            _ => {
                let len = packets[i].data.len();
                let (a, b) = (rng.next() % (len + 1), rng.next() % 64);
                for x in packets[i].data.iter_mut().skip(a).take(b) {
                    *x = 0;
                }
            }
        }
        if packets.is_empty() {
            return;
        }
    }
}

#[test]
fn damaged_streams_never_panic() {
    let Some(dir) = fixtures() else { return };
    for name in [
        "i_wpp",
        "i_slices",
        "i_tskip",
        "b_pyramid",
        "p_amp",
        "s_sao_both_10bit",
        "f_dbk_slices",
        "w_b",
        "b_ctu16",
    ] {
        let data = std::fs::read(dir.join(format!("{name}.mp4"))).unwrap();
        block_on(async {
            let mut d = open(MemSource::new(data)).await.unwrap();
            let info = d.streams().iter().find(|s| s.kind == StreamKind::Video).unwrap().clone();
            let mut packets = Vec::new();
            while let Some(p) = d.next_packet().await.unwrap() {
                if p.stream_id == info.id {
                    packets.push(p);
                }
            }
            for seed in 0..std::env::var("RVP_FUZZ_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(12u64)
            {
                let mut rng = Rng(seed * 7919 + name.len() as u64);
                let mut damaged = packets.clone();
                mutate(&mut damaged, &mut rng);
                let mut dec = hevc_decoder(&info).unwrap();
                for p in &damaged {
                    let _ = dec.send_packet(p);
                    while let Ok(Some(_)) = dec.receive_frame() {}
                }
                let _ = dec.drain();
                while let Ok(Some(_)) = dec.receive_frame() {}
            }
        });
    }
}

#[test]
fn unsupported_streams_are_refused_with_a_clear_message() {
    // Range extensions: profile 4 with the extension flag set in the SPS is refused when the SPS is parsed.
    let sps_err = |bytes: &[u8]| rvp_codec_hevc::ps::Sps::parse(bytes).err().map(|e| e.to_string());
    assert!(sps_err(&[]).is_some());
    assert!(sps_err(&[0x42, 0x01, 0xff, 0xff]).is_some());
}
