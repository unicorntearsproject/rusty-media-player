//! The HEVC stream layer on real x265 streams (MP4 and MKV, Main and Main 10, B-frames): parameter sets and slice headers parse, the
//! pictures come out in presentation order with the container's times, and the reference sets always find their pictures. A backend that
//! decodes nothing stands in for the pixels.
use rvp_codec_hevc::ps::Sps;
use rvp_codec_hevc::stream::{Backend, HevcStream, Picture};
use rvp_core::task::block_on;
use rvp_core::{ColorMatrix, ColorRange, PixelFormat, StreamKind, VideoFrame};
use rvp_demux::{Demuxer, open};
use rvp_host::mock::MemSource;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixtures() -> Option<PathBuf> {
    static ONCE: Once = Once::new();
    if std::env::var_os("RVP_SKIP_FIXTURES").is_some() {
        return None;
    }
    let dir =
        std::env::var_os("RVP_FIXTURES").map(PathBuf::from).unwrap_or_else(|| root().join("target/fixtures"));
    ONCE.call_once(|| {
        let st = Command::new("bash")
            .arg(root().join("tools/gen-fixtures.sh"))
            .arg(&dir)
            .env("RVP_FIXTURE_SET", "hevc")
            .status()
            .expect("gen-fixtures");
        assert!(st.success());
    });
    let d = dir.join("hevc");
    d.join("hevc_aac.mp4").exists().then_some(d)
}

/// Decodes nothing; remembers what it was asked.
#[derive(Default)]
struct Null {
    pictures: usize,
    max_refs: usize,
    slices: usize,
    next: u32,
    pocs: Vec<i32>,
}

impl Backend for Null {
    type Surface = u32;
    fn alloc(&mut self, _: &Sps) -> rvp_codec_hevc::Result<u32> {
        self.next += 1;
        Ok(self.next)
    }
    fn decode(&mut self, p: &Picture<'_, u32>) -> rvp_codec_hevc::Result<()> {
        self.pictures += 1;
        self.max_refs = self.max_refs.max(p.refs.len());
        self.slices += p.slices.len();
        self.pocs.push(p.poc);
        for s in &p.slices {
            assert!(s.ref_lists[0].iter().chain(&s.ref_lists[1]).all(|i| *i < p.refs.len()));
        }
        Ok(())
    }
    fn read(&mut self, _: &u32, sps: &Sps) -> rvp_codec_hevc::Result<VideoFrame> {
        let (w, h) = sps.display_size();
        Ok(VideoFrame {
            width: w,
            height: h,
            format: PixelFormat::Yuv420p8,
            matrix: ColorMatrix::Bt709,
            range: ColorRange::Limited,
            planes: [Vec::new(), Vec::new(), Vec::new()],
            strides: [0; 3],
            pts: 0,
        })
    }
}

fn run(path: &Path) -> (Vec<i64>, Null, Vec<i64>) {
    let data = std::fs::read(path).unwrap();
    block_on(async {
        let mut d = open(MemSource::new(data)).await.unwrap();
        let info = d.streams().iter().find(|s| s.kind == StreamKind::Video).unwrap().clone();
        assert_eq!(info.codec, "hevc");
        let mut s = HevcStream::new(Null::default(), &info.extra_data).unwrap();
        let mut input_pts = Vec::new();
        let mut out = Vec::new();
        while let Some(p) = d.next_packet().await.unwrap() {
            if p.stream_id != info.id {
                continue;
            }
            input_pts.push(p.pts);
            s.push_sample(&p.data, p.pts).unwrap();
            while let Some(f) = s.receive() {
                out.push(f.pts);
            }
        }
        s.drain().unwrap();
        while let Some(f) = s.receive() {
            out.push(f.pts);
        }
        input_pts.sort();
        let null = std::mem::take(s.backend());
        (out, null, input_pts)
    })
}

#[test]
fn x265_streams_come_out_whole_and_in_presentation_order() {
    let Some(dir) = fixtures() else { return };
    for f in ["hevc_aac.mp4", "hevc10_aac.mp4", "hevc_flac.mkv"] {
        let (out, null, input) = run(&dir.join(f));
        assert_eq!(out.len(), 100, "{f}: every picture is shown once");
        assert!(out.windows(2).all(|w| w[0] < w[1]), "{f}: in presentation order: {out:?}");
        assert_eq!(out, input, "{f}: with the container's times");
        assert_eq!(null.pictures, 100);
        assert!(null.max_refs >= 1, "{f}: P and B pictures found their references");
        assert!(null.slices >= 100);
    }
}

#[test]
#[ignore]
fn print_sps() {
    let Ok(path) = std::env::var("RVP_HEVC_FILE") else { return };
    let data = std::fs::read(&path).unwrap();
    block_on(async {
        let d = open(MemSource::new(data)).await.unwrap();
        let info = d.streams().iter().find(|s| s.kind == StreamKind::Video).unwrap().clone();
        for (k, u) in rvp_codec_hevc::ps::hvcc_units(&info.extra_data) {
            if k == 33 {
                let (mut r, mut rem) = (Vec::new(), Vec::new());
                rvp_codec_hevc::nal::unescape(&u[2..], &mut r, &mut rem);
                let s = Sps::parse(&r).unwrap();
                println!(
                    "th_inter={} th_intra={} log2 cb {}..{} tb {}..{} amp={} sao={} tmvp={}",
                    s.max_th_depth_inter,
                    s.max_th_depth_intra,
                    s.log2_min_cb,
                    s.log2_ctb,
                    s.log2_min_tb,
                    s.log2_max_tb,
                    s.amp_enabled,
                    s.sao_enabled,
                    s.temporal_mvp_enabled
                );
            }
        }
    });
}
