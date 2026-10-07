//! Streams built here with a small HEVC writer, to reach what no off-the-shelf encoder gives us: tiles, WPP together with tiles, slice
//! and slice segment boundaries in every place the standard allows, entry points and PCM. The pictures are made of 16x16 coding units,
//! each either PCM with random samples or intra predicted from its neighbours (no residual), so the prediction across tile and slice
//! borders matters. ffmpeg decodes the same bytes and every sample must agree.
use rvp_codec_hevc::cabac::{RANGE_TAB_LPS, TRANS_IDX_LPS, TRANS_IDX_MPS};
use rvp_codec_hevc::ctx;
use rvp_codec_hevc::stream::HevcStream;
use rvp_codec_hevc::sw::SwBackend;
use std::process::{Command, Stdio};

// ---- bits and NAL units ------------------------------------------------------------------------------------------------------

#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    n: usize,
}

impl Bits {
    fn bit(&mut self, b: bool) {
        if self.n % 8 == 0 {
            self.bytes.push(0);
        }
        if b {
            *self.bytes.last_mut().unwrap() |= 0x80 >> (self.n % 8);
        }
        self.n += 1;
    }
    fn bits(&mut self, v: u32, n: u32) {
        for i in (0..n).rev() {
            self.bit((v >> i) & 1 == 1);
        }
    }
    fn ue(&mut self, v: u32) {
        let x = v + 1;
        let len = 32 - x.leading_zeros();
        self.bits(0, len - 1);
        self.bits(x, len);
    }
    fn se(&mut self, v: i32) {
        self.ue(if v > 0 { 2 * v as u32 - 1 } else { (-2 * v) as u32 });
    }
    fn trailing(&mut self) {
        self.bit(true);
        while self.n % 8 != 0 {
            self.bit(false);
        }
    }
    fn align_zero(&mut self) {
        while self.n % 8 != 0 {
            self.bit(false);
        }
    }
}

fn escape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut zeros = 0;
    for &b in data {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

fn nal(kind: u8, payload_escaped: &[u8]) -> Vec<u8> {
    let mut v = vec![0, 0, 0, 1, kind << 1, 1];
    v.extend_from_slice(payload_escaped);
    v
}

fn profile_tier_level(b: &mut Bits) {
    b.bits(0, 2);
    b.bit(false);
    b.bits(1, 5);
    b.bits(0x6000_0000, 32); // compatibility flags 1 and 2
    b.bits(0b1001, 4); // progressive, frame only
    b.bits(0, 32);
    b.bits(0, 12); // 44 reserved bits
    b.bits(93, 8);
}

// ---- the arithmetic encoder --------------------------------------------------------------------------------------------------

struct Enc {
    low: u32,
    range: u32,
    first: bool,
    outstanding: u32,
    out: Bits,
    state: [u8; ctx::COUNT],
}

impl Enc {
    fn new() -> Self {
        Self { low: 0, range: 510, first: true, outstanding: 0, out: Bits::default(), state: [0; ctx::COUNT] }
    }
    fn init_contexts(&mut self, qp: i32) {
        for i in 0..ctx::COUNT {
            let v = ctx::INIT_VALUES[0][i] as i32;
            let (m, n) = ((v >> 4) * 5 - 45, ((v & 15) << 3) - 16);
            let pre = (((m * qp.clamp(0, 51)) >> 4) + n).clamp(1, 126);
            self.state[i] = if pre <= 63 { ((63 - pre) as u8) << 1 } else { (((pre - 64) as u8) << 1) | 1 };
        }
    }
    fn restart(&mut self) {
        self.low = 0;
        self.range = 510;
        self.first = true;
        self.outstanding = 0;
    }
    fn put(&mut self, b: u32) {
        if self.first {
            self.first = false;
        } else {
            self.out.bit(b != 0);
        }
        while self.outstanding > 0 {
            self.out.bit(b == 0);
            self.outstanding -= 1;
        }
    }
    fn renorm(&mut self) {
        while self.range < 256 {
            if self.low < 256 {
                self.put(0);
            } else if self.low >= 512 {
                self.low -= 512;
                self.put(1);
            } else {
                self.low -= 256;
                self.outstanding += 1;
            }
            self.range <<= 1;
            self.low <<= 1;
        }
    }
    fn bin(&mut self, idx: usize, bin: u32) {
        let st = self.state[idx];
        let (p, mps) = ((st >> 1) as usize, (st & 1) as u32);
        let lps = RANGE_TAB_LPS[p][((self.range >> 6) & 3) as usize] as u32;
        self.range -= lps;
        if bin != mps {
            self.low += self.range;
            self.range = lps;
            self.state[idx] = (TRANS_IDX_LPS[p] << 1) | if p == 0 { 1 - mps } else { mps } as u8;
        } else {
            self.state[idx] = (TRANS_IDX_MPS[p] << 1) | mps as u8;
        }
        self.renorm();
    }
    fn bypass(&mut self, bin: u32) {
        self.low <<= 1;
        if bin != 0 {
            self.low += self.range;
        }
        if self.low >= 1024 {
            self.put(1);
            self.low -= 1024;
        } else if self.low < 512 {
            self.put(0);
        } else {
            self.low -= 512;
            self.outstanding += 1;
        }
    }
    fn terminate(&mut self, bin: u32) {
        self.range -= 2;
        if bin != 0 {
            self.low += self.range;
            self.range = 2;
            self.renorm();
            self.put((self.low >> 9) & 1);
            self.out.bits(((self.low >> 7) & 3) | 1, 2);
        } else {
            self.renorm();
        }
    }
}

// ---- the stream --------------------------------------------------------------------------------------------------------------

#[derive(Clone)]
struct Config {
    name: &'static str,
    w_ctb: usize,
    h_ctb: usize,
    /// Tile column widths and row heights in CTBs (one each for no tiles).
    cols: Vec<usize>,
    rows: Vec<usize>,
    uniform: bool,
    wpp: bool,
    /// Slice segment starts in tile scan order: (ts address, dependent).
    segments: Vec<(usize, bool)>,
    pcm: bool,
    pictures: usize,
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
}

struct Layout {
    rs_to_ts: Vec<usize>,
    ts_to_rs: Vec<usize>,
    tile_of_rs: Vec<usize>,
    tile_x0: Vec<usize>,
}

fn layout(c: &Config) -> Layout {
    let n = c.w_ctb * c.h_ctb;
    let mut col_bd = vec![0];
    for w in &c.cols {
        col_bd.push(col_bd.last().unwrap() + w);
    }
    let mut row_bd = vec![0];
    for h in &c.rows {
        row_bd.push(row_bd.last().unwrap() + h);
    }
    let (mut rs_to_ts, mut ts_to_rs, mut tile_of_rs) = (vec![0; n], vec![0; n], vec![0; n]);
    let mut ts = 0;
    for ty in 0..c.rows.len() {
        for tx in 0..c.cols.len() {
            for y in row_bd[ty]..row_bd[ty + 1] {
                for x in col_bd[tx]..col_bd[tx + 1] {
                    let rs = y * c.w_ctb + x;
                    rs_to_ts[rs] = ts;
                    ts_to_rs[ts] = rs;
                    tile_of_rs[rs] = ty * c.cols.len() + tx;
                    ts += 1;
                }
            }
        }
    }
    let tile_x0 = (0..c.rows.len() * c.cols.len()).map(|t| col_bd[t % c.cols.len()]).collect();
    Layout { rs_to_ts, ts_to_rs, tile_of_rs, tile_x0 }
}

fn vps() -> Vec<u8> {
    let mut b = Bits::default();
    b.bits(0, 4);
    b.bits(0b11, 2);
    b.bits(0, 6);
    b.bits(0, 3);
    b.bit(true);
    b.bits(0xffff, 16);
    profile_tier_level(&mut b);
    b.bit(true);
    b.ue(0);
    b.ue(0);
    b.ue(0);
    b.bits(0, 6);
    b.ue(0);
    b.bit(false);
    b.bit(false);
    b.trailing();
    nal(32, &escape(&b.bytes))
}

fn sps(c: &Config) -> Vec<u8> {
    let mut b = Bits::default();
    b.bits(0, 4);
    b.bits(0, 3);
    b.bit(true);
    profile_tier_level(&mut b);
    b.ue(0); // sps id
    b.ue(1); // 4:2:0
    b.ue(c.w_ctb as u32 * 16);
    b.ue(c.h_ctb as u32 * 16);
    b.bit(false); // conformance window
    b.ue(0);
    b.ue(0); // 8 bits
    b.ue(0); // log2_max_poc_lsb - 4
    b.bit(true);
    b.ue(0);
    b.ue(0);
    b.ue(0);
    b.ue(1); // min cb 16
    b.ue(0); // ctb 16
    b.ue(0); // min tb 4
    b.ue(2); // max tb 16
    b.ue(0);
    b.ue(0); // transform hierarchy depths
    b.bit(false); // scaling lists
    b.bit(false); // amp
    b.bit(false); // sao
    b.bit(c.pcm);
    if c.pcm {
        b.bits(7, 4);
        b.bits(7, 4);
        b.ue(1); // min pcm 16
        b.ue(0);
        b.bit(true); // pcm loop filter disabled
    }
    b.ue(0); // short-term RPS
    b.bit(false); // long-term
    b.bit(false); // temporal mvp
    b.bit(false); // strong intra smoothing
    b.bit(false); // vui
    b.bit(false); // extension
    b.trailing();
    nal(33, &escape(&b.bytes))
}

fn pps(c: &Config) -> Vec<u8> {
    let tiles = c.cols.len() > 1 || c.rows.len() > 1;
    let mut b = Bits::default();
    b.ue(0);
    b.ue(0);
    b.bit(c.segments.iter().any(|s| s.1)); // dependent slice segments
    b.bit(false);
    b.bits(0, 3);
    b.bit(false);
    b.bit(false);
    b.ue(0);
    b.ue(0);
    b.se(0);
    b.bit(false);
    b.bit(false);
    b.bit(false); // cu_qp_delta
    b.se(0);
    b.se(0);
    b.bit(false);
    b.bit(false);
    b.bit(false);
    b.bit(false); // transquant bypass
    b.bit(tiles);
    b.bit(c.wpp);
    if tiles {
        b.ue(c.cols.len() as u32 - 1);
        b.ue(c.rows.len() as u32 - 1);
        b.bit(c.uniform);
        if !c.uniform {
            for w in &c.cols[..c.cols.len() - 1] {
                b.ue(*w as u32 - 1);
            }
            for h in &c.rows[..c.rows.len() - 1] {
                b.ue(*h as u32 - 1);
            }
        }
        b.bit(false); // loop filter across tiles
    }
    b.bit(false); // loop filter across slices
    b.bit(true); // deblocking control present
    b.bit(false);
    b.bit(true); // deblocking disabled
    b.bit(false); // scaling list
    b.bit(false);
    b.ue(0);
    b.bit(false);
    b.bit(false);
    b.trailing();
    nal(34, &escape(&b.bytes))
}

/// The slice segments of one IDR picture.
fn picture(c: &Config, lay: &Layout, rng: &mut Rng) -> Vec<Vec<u8>> {
    let n = c.w_ctb * c.h_ctb;
    let tiles = c.cols.len() > 1 || c.rows.len() > 1;
    let addr_bits = (usize::BITS - (n - 1).leading_zeros()) as u32;
    let addr_bits = if n <= 1 { 0 } else { addr_bits };
    let mut units = Vec::new();
    let mut slice_addr_rs = vec![usize::MAX; n];
    let mut wpp_state: Option<[u8; ctx::COUNT]> = None;
    let mut ds_state: Option<[u8; ctx::COUNT]> = None;
    let mut cur_slice_rs = 0usize;
    for (si, &(start, dependent)) in c.segments.iter().enumerate() {
        let end = c.segments.get(si + 1).map_or(n, |s| s.0);
        let first_rs = lay.ts_to_rs[start];
        if !dependent {
            cur_slice_rs = first_rs;
        }
        // The slice data: substreams, each ending in a flush.
        let mut subs: Vec<Vec<u8>> = Vec::new();
        let mut e = Enc::new();
        for ts in start..end {
            let rs = lay.ts_to_rs[ts];
            let (x, y) = (rs % c.w_ctb, rs / c.w_ctb);
            slice_addr_rs[rs] = cur_slice_rs;
            let tile_start = ts == 0 || lay.tile_of_rs[rs] != lay.tile_of_rs[lay.ts_to_rs[ts - 1]];
            let row_start = c.wpp && x == lay.tile_x0[lay.tile_of_rs[rs]];
            if tile_start {
                e.init_contexts(26);
            } else if row_start {
                let tr = if y > 0 && x + 1 < c.w_ctb { Some((y - 1) * c.w_ctb + x + 1) } else { None };
                let avail = tr.is_some_and(|t| {
                    slice_addr_rs[t] == cur_slice_rs && lay.tile_of_rs[t] == lay.tile_of_rs[rs]
                });
                match (avail, wpp_state) {
                    (true, Some(s)) => e.state = s,
                    _ => e.init_contexts(26),
                }
            } else if ts == start {
                match (dependent, ds_state) {
                    (true, Some(s)) => e.state = s,
                    _ => e.init_contexts(26),
                }
            }
            // The coding unit.
            e.bin(ctx::PART_MODE, 1); // 2Nx2N
            let pcm = c.pcm && rng.next() % 3 == 0;
            if c.pcm {
                e.terminate(pcm as u32);
            }
            if pcm {
                e.out.align_zero();
                for _ in 0..16 * 16 + 2 * 8 * 8 {
                    let v = rng.next() as u8;
                    e.out.bits(v as u32, 8);
                }
                e.restart();
            } else {
                e.bin(ctx::PREV_INTRA_LUMA, 0);
                let rem = rng.next() % 32;
                for i in (0..5).rev() {
                    e.bypass((rem >> i) & 1);
                }
                let chroma = rng.next() % 5;
                if chroma == 4 {
                    e.bin(ctx::INTRA_CHROMA, 0);
                } else {
                    e.bin(ctx::INTRA_CHROMA, 1);
                    e.bypass(chroma >> 1);
                    e.bypass(chroma & 1);
                }
                e.bin(ctx::CBF_CHROMA, 0);
                e.bin(ctx::CBF_CHROMA, 0);
                e.bin(ctx::CBF_LUMA + 1, 0);
            }
            if c.wpp && x == lay.tile_x0[lay.tile_of_rs[rs]] + 1 {
                wpp_state = Some(e.state);
            }
            let last = ts + 1 == end;
            e.terminate(last as u32);
            if last {
                ds_state = Some(e.state);
                break;
            }
            let nrs = lay.ts_to_rs[ts + 1];
            let next_tile = tiles && lay.tile_of_rs[nrs] != lay.tile_of_rs[rs];
            let next_row = c.wpp && nrs % c.w_ctb == lay.tile_x0[lay.tile_of_rs[nrs]];
            if next_tile || next_row {
                e.terminate(1);
                e.out.align_zero();
                subs.push(core::mem::take(&mut e.out.bytes));
                e.out.n = 0;
                e.restart();
            }
        }
        e.out.align_zero();
        subs.push(core::mem::take(&mut e.out.bytes));
        // The header.
        let mut h = Bits::default();
        h.bit(start == 0);
        h.bit(false); // no_output_of_prior_pics
        h.ue(0);
        if start != 0 {
            if c.segments.iter().any(|s| s.1) {
                h.bit(dependent);
            }
            h.bits(first_rs as u32, addr_bits);
        }
        if !dependent {
            h.ue(2); // I slice
            h.se(0); // slice_qp_delta
        }
        if tiles || c.wpp {
            h.ue(subs.len() as u32 - 1);
            if subs.len() > 1 {
                let escaped: Vec<usize> = subs[..subs.len() - 1].iter().map(|s| escape(s).len()).collect();
                let max = escaped.iter().max().copied().unwrap();
                let len = (usize::BITS - (max - 1).leading_zeros()).max(1);
                h.ue(len - 1);
                for sz in escaped {
                    h.bits(sz as u32 - 1, len);
                }
            }
        }
        h.trailing();
        let mut payload = escape(&h.bytes);
        for s in &subs {
            payload.extend_from_slice(&escape(s));
        }
        units.push(nal(19, &payload));
    }
    units
}

fn build(c: &Config, seed: u64) -> (Vec<u8>, Vec<Vec<Vec<u8>>>) {
    let lay = layout(c);
    let mut rng = Rng(seed);
    let mut all = Vec::new();
    all.extend(vps());
    all.extend(sps(c));
    all.extend(pps(c));
    let mut pics = Vec::new();
    for _ in 0..c.pictures {
        let p = picture(c, &lay, &mut rng);
        for u in &p {
            all.extend_from_slice(u);
        }
        pics.push(p);
    }
    (all, pics)
}

fn ours(c: &Config, pics: &[Vec<Vec<u8>>]) -> Result<Vec<u8>, String> {
    let (w, h) = (c.w_ctb * 16, c.h_ctb * 16);
    let strip = |u: &[u8]| u[4..].to_vec();
    let mut s = HevcStream::new(SwBackend::new(), &[]).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut take = |s: &mut HevcStream<SwBackend>, out: &mut Vec<u8>| {
        while let Some(f) = s.receive() {
            assert_eq!((f.width as usize, f.height as usize), (w, h));
            for (i, p) in f.planes.iter().enumerate() {
                let (pw, ph) = if i == 0 { (w, h) } else { (w / 2, h / 2) };
                for y in 0..ph {
                    out.extend_from_slice(&p[y * f.strides[i]..y * f.strides[i] + pw]);
                }
            }
        }
    };
    let mut first = true;
    for p in pics {
        let mut sample = Vec::new();
        let mut units: Vec<Vec<u8>> = Vec::new();
        if first {
            units.extend([vps(), sps(c), pps(c)]);
            first = false;
        }
        units.extend(p.iter().cloned());
        for u in &units {
            let body = strip(u);
            sample.extend_from_slice(&(body.len() as u32).to_be_bytes());
            sample.extend_from_slice(&body);
        }
        s.push_sample(&sample, 0).map_err(|e| e.to_string())?;
        take(&mut s, &mut out);
    }
    s.drain().map_err(|e| e.to_string())?;
    take(&mut s, &mut out);
    Ok(out)
}

fn ffmpeg(stream: &[u8], tag: &str) -> Vec<u8> {
    let path = std::env::temp_dir().join(format!("rvp-structure-{tag}-{}.hevc", std::process::id()));
    std::fs::write(&path, stream).unwrap();
    let o = Command::new("ffmpeg")
        .args(["-v", "error", "-f", "hevc", "-i"])
        .arg(&path)
        .args(["-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])
        .stderr(Stdio::inherit())
        .output()
        .expect("ffmpeg");
    let _ = std::fs::remove_file(&path);
    assert!(o.status.success(), "ffmpeg could not decode {tag}");
    o.stdout
}

fn configs() -> Vec<Config> {
    let base = Config {
        name: "",
        w_ctb: 7,
        h_ctb: 5,
        cols: vec![7],
        rows: vec![5],
        uniform: true,
        wpp: false,
        segments: vec![(0, false)],
        pcm: true,
        pictures: 2,
    };
    let mut v = Vec::new();
    let mut add = |name: &'static str, f: &dyn Fn(&mut Config)| {
        let mut c = base.clone();
        c.name = name;
        f(&mut c);
        v.push(c);
    };
    add("plain", &|_| {});
    add("no_pcm", &|c| c.pcm = false);
    add("wpp", &|c| c.wpp = true);
    add("slices_by_row", &|c| c.segments = vec![(0, false), (7, false), (21, false)]);
    add("slices_mid_row", &|c| c.segments = vec![(0, false), (10, false), (17, false), (30, false)]);
    add("dependent_segments", &|c| {
        c.segments = vec![(0, false), (5, true), (12, true), (20, false), (27, true)]
    });
    add("wpp_slices", &|c| {
        c.wpp = true;
        // With WPP a slice that starts inside a row has to end in it.
        c.segments = vec![(0, false), (7, false), (10, false), (14, false), (21, false)];
    });
    add("wpp_dependent", &|c| {
        c.wpp = true;
        c.segments = vec![(0, false), (7, true), (10, true), (14, true), (21, false), (24, true), (28, true)];
    });
    add("tiles_2x2", &|c| {
        c.cols = vec![3, 4];
        c.rows = vec![2, 3];
        c.uniform = false;
    });
    add("tiles_uniform_3x2", &|c| {
        c.cols = vec![2, 2, 3];
        c.rows = vec![2, 3];
        c.uniform = true;
    });
    add("tiles_one_slice_each", &|c| {
        c.cols = vec![3, 4];
        c.rows = vec![2, 3];
        c.uniform = false;
        // Tile scan: tile 0 has 6 CTBs, tile 1 has 8, tile 2 has 9, tile 3 has 12.
        c.segments = vec![(0, false), (6, false), (14, false), (23, false)];
    });
    add("tiles_with_wpp", &|c| {
        c.cols = vec![3, 4];
        c.rows = vec![2, 3];
        c.uniform = false;
        c.wpp = true;
    });
    add("tiles_wpp_slices", &|c| {
        c.cols = vec![3, 4];
        c.rows = vec![2, 3];
        c.uniform = false;
        c.wpp = true;
        c.segments = vec![(0, false), (6, false), (14, false), (23, false)];
    });
    add("tiles_dependent_in_tile", &|c| {
        c.cols = vec![3, 4];
        c.rows = vec![2, 3];
        c.uniform = false;
        c.segments =
            vec![(0, false), (3, true), (6, false), (10, true), (14, false), (23, false), (29, true)];
    });
    add("one_ctb_wide_tiles", &|c| {
        c.cols = vec![1, 1, 5];
        c.rows = vec![5];
        c.uniform = false;
        c.wpp = true;
    });
    add("tiles_columns_only", &|c| {
        c.cols = vec![2, 5];
        c.rows = vec![5];
        c.uniform = false;
    });
    v
}

#[test]
fn tiles_wpp_slices_and_pcm_decode_like_ffmpeg() {
    if std::env::var_os("RVP_SKIP_FIXTURES").is_some()
        || Command::new("ffmpeg").arg("-version").output().is_err()
    {
        return;
    }
    let mut failures = Vec::new();
    for (i, c) in configs().iter().enumerate() {
        let (stream, pics) = build(c, 0x5eed + i as u64);
        // ffmpeg numbers the WPP rows by position in the tile scan, which is wrong with tiles; the picture does not depend on WPP, so
        // its decode of the same content without WPP is the reference.
        let want = if c.wpp && c.cols.len() * c.rows.len() > 1 {
            let mut twin = c.clone();
            twin.wpp = false;
            ffmpeg(&build(&twin, 0x5eed + i as u64).0, c.name)
        } else {
            ffmpeg(&stream, c.name)
        };
        match ours(c, &pics) {
            Err(e) => failures.push(format!("{}: {e}", c.name)),
            Ok(got) if got.len() != want.len() => {
                failures.push(format!("{}: {} bytes, ffmpeg {}", c.name, got.len(), want.len()))
            }
            Ok(got) => {
                if let Some(p) = got.iter().zip(&want).position(|(a, b)| a != b) {
                    let (w, h) = (c.w_ctb * 16, c.h_ctb * 16);
                    let fs = w * h * 3 / 2;
                    let (f, o) = (p / fs, p % fs);
                    if std::env::var_os("RVP_MAP").is_some() {
                        for cy in 0..c.h_ctb {
                            let row: String = (0..c.w_ctb)
                                .map(|cx| {
                                    if (0..16).any(|j| {
                                        got[(cy * 16 + j) * w + cx * 16..(cy * 16 + j) * w + cx * 16 + 16]
                                            != want[(cy * 16 + j) * w + cx * 16
                                                ..(cy * 16 + j) * w + cx * 16 + 16]
                                    }) {
                                        'X'
                                    } else {
                                        '.'
                                    }
                                })
                                .collect();
                            eprintln!("{} {row}", c.name);
                        }
                    }
                    failures.push(format!(
                        "{}: differs in picture {f} at {} ({} of the picture's {} bytes differ in all)",
                        c.name,
                        if o < w * h {
                            format!("luma ({},{})", o % w, o / w)
                        } else {
                            format!("chroma byte {}", o - w * h)
                        },
                        got.iter().zip(&want).filter(|(a, b)| a != b).count(),
                        fs
                    ));
                } else {
                    eprintln!("ok   {}", c.name);
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
