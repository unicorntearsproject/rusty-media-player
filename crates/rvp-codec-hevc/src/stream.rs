//! From samples to pictures: parameter sets, picture boundaries, POC, the DPB and output order, around a [`Backend`] that decodes the
//! pixels (a hardware decoder, or a software one later).
use crate::dpb::{Dpb, DpbEntry, Marking, PocState, RefIdx, RefSet, Rps, build_ref_lists, derive_rps};
use crate::nal::{self, NalHeader, kind};
use crate::ps::{Pps, Sps};
use crate::slice::SliceHeader;
use crate::{Error, Result};
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use rvp_core::VideoFrame;

/// A reference picture of the picture being decoded.
pub struct RefPic<'a, S> {
    /// The decoded picture.
    pub surface: &'a S,
    /// Its POC.
    pub poc: i32,
    /// Marked as long-term.
    pub long_term: bool,
}

/// One slice segment of the picture.
pub struct SliceSeg<'a> {
    /// Its header.
    pub header: SliceHeader,
    /// The whole NAL unit (two header bytes included, emulation prevention bytes still in).
    pub unit: &'a [u8],
    /// `RefPicList0` and `RefPicList1` as indices into [`Picture::refs`].
    pub ref_lists: [Vec<RefIdx>; 2],
}

/// What a backend gets to decode one picture.
pub struct Picture<'a, S> {
    /// The sequence parameter set.
    pub sps: &'a Sps,
    /// The picture parameter set.
    pub pps: &'a Pps,
    /// `PicOrderCntVal`.
    pub poc: i32,
    /// The header of the first slice segment.
    pub header: &'a SliceHeader,
    /// Where the picture goes.
    pub surface: &'a S,
    /// The reference pictures the picture may use (the RPS, found in the buffer).
    pub refs: Vec<RefPic<'a, S>>,
    /// `RefPicSetStCurrBefore`, `RefPicSetStCurrAfter` and `RefPicSetLtCurr` as indices into `refs`.
    pub rps: [Vec<RefIdx>; 3],
    /// The slice segments, in order.
    pub slices: Vec<SliceSeg<'a>>,
}

/// What decodes the pixels.
pub trait Backend {
    /// A decoded picture (a surface handle, a frame buffer): cheap to clone.
    type Surface: Clone;
    /// A picture to decode into, for a stream with these parameters.
    fn alloc(&mut self, sps: &Sps) -> Result<Self::Surface>;
    /// Decode one picture.
    fn decode(&mut self, pic: &Picture<'_, Self::Surface>) -> Result<()>;
    /// The finished picture as a frame (cropped to the conformance window, `pts` not yet set).
    fn read(&mut self, surface: &Self::Surface, sps: &Sps) -> Result<VideoFrame>;
}

struct Pending {
    units: Vec<Vec<u8>>,
    headers: Vec<SliceHeader>,
    pts: i64,
}

/// A stream of HEVC pictures going through a backend.
pub struct HevcStream<B: Backend> {
    backend: B,
    sps: [Option<Sps>; 16],
    pps: [Option<Pps>; 64],
    length_size: usize,
    poc: PocState,
    dpb: Dpb<B::Surface>,
    out: VecDeque<VideoFrame>,
    pending: Option<Pending>,
    /// No picture decoded since the start or a flush: the next IRAP has `NoRaslOutputFlag` set.
    fresh: bool,
    after_eos: bool,
    /// `NoRaslOutputFlag` of the last IRAP: its RASL pictures are skipped when set.
    skip_rasl: bool,
    active_sps: Option<u8>,
}

impl<B: Backend> HevcStream<B> {
    /// A stream whose samples are length-prefixed units; `hvcc` is the `hvcC` record (parameter sets and the length size).
    pub fn new(backend: B, hvcc: &[u8]) -> Result<Self> {
        let mut s = Self {
            backend,
            sps: Default::default(),
            pps: core::array::from_fn(|_| None),
            length_size: 4,
            poc: PocState::default(),
            dpb: Dpb::new(),
            out: VecDeque::new(),
            pending: None,
            fresh: true,
            after_eos: false,
            skip_rasl: false,
            active_sps: None,
        };
        if hvcc.len() >= 23 {
            s.length_size = (hvcc[21] & 3) as usize + 1;
            let mut at = 23;
            for _ in 0..hvcc[22] {
                let Some(n) = hvcc.get(at + 1..at + 3).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize)
                else {
                    break;
                };
                at += 3;
                for _ in 0..n {
                    let Some(len) = hvcc.get(at..at + 2).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize)
                    else {
                        break;
                    };
                    at += 2;
                    if let Some(u) = hvcc.get(at..at + len) {
                        s.parameter_set(u)?;
                    }
                    at += len;
                }
            }
        }
        Ok(s)
    }

    /// The backend.
    pub fn backend(&mut self) -> &mut B {
        &mut self.backend
    }

    fn parameter_set(&mut self, unit: &[u8]) -> Result<()> {
        let Some(h) = NalHeader::parse(unit) else { return Ok(()) };
        if h.layer != 0 {
            return Ok(());
        }
        let mut rbsp = Vec::new();
        let mut rem = Vec::new();
        nal::unescape(&unit[2..], &mut rbsp, &mut rem);
        match h.kind {
            kind::SPS => {
                let sps = Sps::parse(&rbsp)?;
                let id = sps.id as usize;
                self.sps[id] = Some(sps);
            }
            kind::PPS => {
                let pps = Pps::parse(&rbsp)?;
                let id = pps.id as usize;
                self.pps[id] = Some(pps);
            }
            _ => {}
        }
        Ok(())
    }

    /// Feed one sample (the units of one access unit, length-prefixed) with its presentation time.
    pub fn push_sample(&mut self, data: &[u8], pts: i64) -> Result<()> {
        let mut first_err = None;
        for unit in nal::split_length_prefixed(data, self.length_size) {
            if let Err(e) = self.push_unit(unit, pts) {
                first_err.get_or_insert(e);
            }
        }
        if let Err(e) = self.finish_picture() {
            first_err.get_or_insert(e);
        }
        first_err.map_or(Ok(()), Err)
    }

    fn push_unit(&mut self, unit: &[u8], pts: i64) -> Result<()> {
        let Some(h) = NalHeader::parse(unit) else { return Ok(()) };
        if h.layer != 0 {
            return Ok(());
        }
        match h.kind {
            kind::SPS | kind::PPS => {
                // A parameter set between pictures: the picture before it is complete.
                self.finish_picture()?;
                self.parameter_set(unit)
            }
            kind::EOS | kind::EOB => {
                self.finish_picture()?;
                self.after_eos = true;
                Ok(())
            }
            _ if h.is_slice() => {
                let pending_prev =
                    self.pending.as_ref().and_then(|p| p.headers.last().filter(|h| !h.dependent).cloned());
                let header = {
                    let (sps, pps) = (&self.sps, &self.pps);
                    let lookup = |id: u8| -> Option<(Pps, Sps)> {
                        let p = pps[id as usize].clone()?;
                        let s = sps[p.sps_id as usize].clone()?;
                        Some((p, s))
                    };
                    SliceHeader::parse(unit, &lookup, pending_prev.as_ref())?
                };
                if header.first_slice_segment_in_pic {
                    self.finish_picture()?;
                    self.pending = Some(Pending { units: Vec::new(), headers: Vec::new(), pts });
                }
                match &mut self.pending {
                    Some(p) => {
                        p.units.push(unit.to_vec());
                        p.headers.push(header);
                        Ok(())
                    }
                    // A slice of a picture whose first slice was lost: nothing to attach it to.
                    None => Ok(()),
                }
            }
            _ => Ok(()),
        }
    }

    /// Everything the stream has decoded so far, in output order.
    pub fn receive(&mut self) -> Option<VideoFrame> {
        self.out.pop_front()
    }

    /// Frames waiting to be taken.
    pub fn frames_ready(&self) -> usize {
        self.out.len()
    }

    /// End of the stream: output every picture still waiting.
    pub fn drain(&mut self) -> Result<()> {
        self.finish_picture()?;
        while self.dpb.waiting() > 0 {
            self.bump()?;
        }
        Ok(())
    }

    /// A seek: forget everything but the parameter sets. The next picture must be an IRAP.
    pub fn flush(&mut self) {
        self.pending = None;
        self.dpb = Dpb::new();
        self.out.clear();
        self.poc = PocState::default();
        self.fresh = true;
        self.after_eos = false;
    }

    fn bump(&mut self) -> Result<()> {
        let Some(i) = self.dpb.bump_index() else { return Ok(()) };
        let sps_id = self.active_sps.ok_or(Error::Invalid("no active SPS"))?;
        let sps = self.sps[sps_id as usize].clone().ok_or(Error::Invalid("active SPS vanished"))?;
        let surface = self.dpb.entries[i].payload.clone();
        let pts = self.dpb.entries[i].pts;
        let mut frame = self.backend.read(&surface, &sps)?;
        frame.pts = pts;
        self.out.push_back(frame);
        self.dpb.entries[i].needed_for_output = false;
        self.dpb.sweep();
        Ok(())
    }

    fn finish_picture(&mut self) -> Result<()> {
        let Some(p) = self.pending.take() else { return Ok(()) };
        let h0 = p.headers[0].clone();
        let (pps, sps) = {
            let pps = self.pps[h0.pps_id as usize].clone().ok_or(Error::Invalid("PPS vanished"))?;
            let sps = self.sps[pps.sps_id as usize].clone().ok_or(Error::Invalid("SPS vanished"))?;
            (pps, sps)
        };
        let nal = h0.nal;
        let irap_no_rasl = nal.is_irap() && (nal.is_idr() || nal.is_bla() || self.fresh || self.after_eos);
        if nal.is_irap() {
            self.skip_rasl = irap_no_rasl;
        }
        // Until the first IRAP nothing can be decoded; RASL pictures of an IRAP that started the stream are skipped.
        if (self.fresh && !irap_no_rasl) || (nal.is_rasl() && self.skip_rasl) {
            return Ok(());
        }
        let max_lsb = sps.max_poc_lsb;
        let poc = self.poc.poc(h0.poc_lsb, max_lsb, irap_no_rasl);
        let rps: Rps = derive_rps(&h0, poc, max_lsb);
        let first_picture = self.fresh;
        if irap_no_rasl && !first_picture && (nal.is_idr() || nal.is_bla()) && h0.no_output_of_prior_pics {
            // The encoder asked for the pictures before this one not to be shown.
            for e in &mut self.dpb.entries {
                e.needed_for_output = false;
            }
        } else if irap_no_rasl {
            // Show everything that is still waiting, then start over.
            while self.dpb.waiting() > 0 {
                self.bump()?;
            }
        }
        let set: RefSet = self.dpb.apply_rps(&rps, max_lsb, irap_no_rasl);
        self.dpb.sweep();
        // The set's indices into the buffer moved with the sweep: look the references up again by POC.
        let set = relocate(&self.dpb, &set);
        self.active_sps = Some(sps.id);
        while self.dpb.must_bump(
            sps.max_num_reorder_pics as usize,
            sps.max_latency_increase_plus1,
            sps.dpb_size(),
        ) {
            self.bump()?;
        }
        // The decode itself.
        let surface = self.backend.alloc(&sps)?;
        let mut slices = Vec::new();
        let mut prev_lists: [Vec<RefIdx>; 2] = [Vec::new(), Vec::new()];
        for (unit, header) in p.units.iter().zip(&p.headers) {
            let lists = if header.dependent { prev_lists.clone() } else { build_ref_lists(header, &set) };
            if !header.dependent {
                prev_lists = lists.clone();
            }
            if header.slice_type != crate::slice::SliceType::I && lists[0].is_empty() {
                return Err(Error::Invalid("a predicted slice with no reference picture in the buffer"));
            }
            slices.push(SliceSeg { header: header.clone(), unit: unit.as_slice(), ref_lists: lists });
        }
        let refs: Vec<RefPic<'_, B::Surface>> = set
            .pics
            .iter()
            .map(|(i, poc, lt)| RefPic { surface: &self.dpb.entries[*i].payload, poc: *poc, long_term: *lt })
            .collect();
        let pic = Picture {
            sps: &sps,
            pps: &pps,
            poc,
            header: &h0,
            surface: &surface,
            refs,
            rps: [set.st_curr_before.clone(), set.st_curr_after.clone(), set.lt_curr.clone()],
            slices,
        };
        self.backend.decode(&pic)?;
        drop(pic);
        self.fresh = false;
        self.after_eos = false;
        // The picture joins the buffer.
        let output = h0.pic_output_flag && !(nal.is_rasl() && self.skip_rasl);
        self.dpb.count_latency(poc);
        self.dpb.entries.push(DpbEntry {
            poc,
            marking: Marking::ShortTerm,
            needed_for_output: output,
            latency: 0,
            payload: surface,
            pts: p.pts,
        });
        if nal.tid == 0 && !nal.is_rasl() && !nal.is_radl() && !nal.is_sub_layer_non_ref() {
            self.poc.remember(poc, max_lsb);
        }
        while self.dpb.waiting() > sps.max_num_reorder_pics as usize
            || (sps.max_latency_increase_plus1 != 0 && {
                let limit = sps.max_num_reorder_pics as u32 + sps.max_latency_increase_plus1 - 1;
                self.dpb.entries.iter().any(|e| e.needed_for_output && e.latency >= limit)
            })
        {
            self.bump()?;
        }
        Ok(())
    }
}

/// After `sweep` removed entries, the indices of a [`RefSet`] point at the wrong pictures: find each by its POC again.
fn relocate<T>(dpb: &Dpb<T>, set: &RefSet) -> RefSet {
    let mut out = RefSet { missing: set.missing, ..RefSet::default() };
    let mut map: Vec<Option<usize>> = Vec::new();
    for (_, poc, lt) in &set.pics {
        // Long-term and short-term pictures can share nothing: a POC is unique among those still marked.
        let at = dpb.entries.iter().position(|e| e.poc == *poc && e.marking != Marking::Unused);
        match at {
            Some(i) => {
                out.pics.push((i, *poc, *lt));
                map.push(Some(out.pics.len() - 1));
            }
            None => map.push(None),
        }
    }
    let remap = |v: &Vec<RefIdx>| -> Vec<RefIdx> { v.iter().filter_map(|i| map[*i]).collect() };
    out.st_curr_before = remap(&set.st_curr_before);
    out.st_curr_after = remap(&set.st_curr_after);
    out.lt_curr = remap(&set.lt_curr);
    out
}
