//! HEVC through VA-API on Linux. libva is opened with `dlopen` when a stream needs it, so nothing links against it and a machine without
//! it (or without a GPU that decodes HEVC) just says so. The stream layer (`rvp-codec-hevc`) does the parameter sets, reference
//! pictures and output order; this module hands each picture to the GPU as VA-API buffers and reads the finished surface back.
#![allow(unsafe_code)]
use crate::SemiPlanar;
use rvp_codec_hevc::ps::{ScalingList, Sps};
use rvp_codec_hevc::slice::SliceType;
use rvp_codec_hevc::stream::{Backend, HevcStream, Picture};
use rvp_core::{
    Error, Packet, PlatformSupport, PlatformVideo, Result as CoreResult, StreamInfo, VideoDecoder, VideoFrame,
};
use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::rc::Rc;

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(file: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn open(path: *const c_char, flags: c_int, ...) -> c_int;
    fn close(fd: c_int) -> c_int;
}

const RTLD_NOW: c_int = 2;
const O_RDWR: c_int = 2;
const O_CLOEXEC: c_int = 0o2000000;

const VA_STATUS_SUCCESS: c_int = 0;
const VA_INVALID_ID: u32 = 0xffff_ffff;
const VA_PROFILE_HEVC_MAIN: c_int = 17;
const VA_PROFILE_HEVC_MAIN10: c_int = 18;
const VA_ENTRYPOINT_VLD: c_int = 1;
const VA_CONFIG_ATTRIB_RT_FORMAT: c_int = 0;
const VA_RT_FORMAT_YUV420: u32 = 0x1;
const VA_RT_FORMAT_YUV420_10: u32 = 0x100;
const VA_PROGRESSIVE: c_int = 1;
const BUF_PICTURE_PARAMETER: c_int = 0;
const BUF_IQ_MATRIX: c_int = 1;
const BUF_SLICE_PARAMETER: c_int = 4;
const BUF_SLICE_DATA: c_int = 5;
const FOURCC_NV12: u32 = 0x3231_564E;
const FOURCC_P010: u32 = 0x3031_3050;
const PIC_INVALID: u32 = 0x1;
const PIC_LONG_TERM: u32 = 0x8;
const PIC_ST_CURR_BEFORE: u32 = 0x10;
const PIC_ST_CURR_AFTER: u32 = 0x20;
const PIC_LT_CURR: u32 = 0x40;

#[repr(C)]
#[derive(Clone, Copy)]
struct ConfigAttrib {
    kind: c_int,
    value: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ImageFormat {
    fourcc: u32,
    byte_order: u32,
    bits_per_pixel: u32,
    depth: u32,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
    alpha_mask: u32,
    reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Image {
    image_id: u32,
    format: ImageFormat,
    buf: u32,
    width: u16,
    height: u16,
    data_size: u32,
    num_planes: u32,
    pitches: [u32; 3],
    offsets: [u32; 3],
    num_palette_entries: i32,
    entry_bytes: i32,
    component_order: [i8; 4],
    reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PictureHevc {
    picture_id: u32,
    pic_order_cnt: i32,
    flags: u32,
    reserved: [u32; 4],
}

impl PictureHevc {
    const INVALID: Self =
        Self { picture_id: VA_INVALID_ID, pic_order_cnt: 0, flags: PIC_INVALID, reserved: [0; 4] };
}

#[repr(C)]
struct PicParams {
    curr_pic: PictureHevc,
    reference_frames: [PictureHevc; 15],
    pic_width_in_luma_samples: u16,
    pic_height_in_luma_samples: u16,
    pic_fields: u32,
    sps_max_dec_pic_buffering_minus1: u8,
    bit_depth_luma_minus8: u8,
    bit_depth_chroma_minus8: u8,
    pcm_sample_bit_depth_luma_minus1: u8,
    pcm_sample_bit_depth_chroma_minus1: u8,
    log2_min_luma_coding_block_size_minus3: u8,
    log2_diff_max_min_luma_coding_block_size: u8,
    log2_min_transform_block_size_minus2: u8,
    log2_diff_max_min_transform_block_size: u8,
    log2_min_pcm_luma_coding_block_size_minus3: u8,
    log2_diff_max_min_pcm_luma_coding_block_size: u8,
    max_transform_hierarchy_depth_intra: u8,
    max_transform_hierarchy_depth_inter: u8,
    init_qp_minus26: i8,
    diff_cu_qp_delta_depth: u8,
    pps_cb_qp_offset: i8,
    pps_cr_qp_offset: i8,
    log2_parallel_merge_level_minus2: u8,
    num_tile_columns_minus1: u8,
    num_tile_rows_minus1: u8,
    column_width_minus1: [u16; 19],
    row_height_minus1: [u16; 21],
    slice_parsing_fields: u32,
    log2_max_pic_order_cnt_lsb_minus4: u8,
    num_short_term_ref_pic_sets: u8,
    num_long_term_ref_pic_sps: u8,
    num_ref_idx_l0_default_active_minus1: u8,
    num_ref_idx_l1_default_active_minus1: u8,
    pps_beta_offset_div2: i8,
    pps_tc_offset_div2: i8,
    num_extra_slice_header_bits: u8,
    st_rps_bits: u32,
    reserved: [u32; 8],
}

#[repr(C)]
struct SliceParams {
    slice_data_size: u32,
    slice_data_offset: u32,
    slice_data_flag: u32,
    slice_data_byte_offset: u32,
    slice_segment_address: u32,
    ref_pic_list: [[u8; 15]; 2],
    long_slice_flags: u32,
    collocated_ref_idx: u8,
    num_ref_idx_l0_active_minus1: u8,
    num_ref_idx_l1_active_minus1: u8,
    slice_qp_delta: i8,
    slice_cb_qp_offset: i8,
    slice_cr_qp_offset: i8,
    slice_beta_offset_div2: i8,
    slice_tc_offset_div2: i8,
    luma_log2_weight_denom: u8,
    delta_chroma_log2_weight_denom: i8,
    delta_luma_weight_l0: [i8; 15],
    luma_offset_l0: [i8; 15],
    delta_chroma_weight_l0: [[i8; 2]; 15],
    chroma_offset_l0: [[i8; 2]; 15],
    delta_luma_weight_l1: [i8; 15],
    luma_offset_l1: [i8; 15],
    delta_chroma_weight_l1: [[i8; 2]; 15],
    chroma_offset_l1: [[i8; 2]; 15],
    five_minus_max_num_merge_cand: u8,
    num_entry_point_offsets: u16,
    entry_offset_to_subset_array: u16,
    slice_data_num_emu_prevn_bytes: u16,
    reserved: [u32; 2],
}

#[repr(C)]
struct IqMatrix {
    scaling_list_4x4: [[u8; 16]; 6],
    scaling_list_8x8: [[u8; 64]; 6],
    scaling_list_16x16: [[u8; 64]; 6],
    scaling_list_32x32: [[u8; 64]; 2],
    scaling_list_dc_16x16: [u8; 6],
    scaling_list_dc_32x32: [u8; 2],
    reserved: [u32; 4],
}

type Dpy = *mut c_void;

/// The functions of libva and libva-drm, resolved once.
struct VaApi {
    get_display_drm: unsafe extern "C" fn(c_int) -> Dpy,
    initialize: unsafe extern "C" fn(Dpy, *mut c_int, *mut c_int) -> c_int,
    terminate: unsafe extern "C" fn(Dpy) -> c_int,
    query_config_entrypoints: unsafe extern "C" fn(Dpy, c_int, *mut c_int, *mut c_int) -> c_int,
    max_num_entrypoints: unsafe extern "C" fn(Dpy) -> c_int,
    get_config_attributes: unsafe extern "C" fn(Dpy, c_int, c_int, *mut ConfigAttrib, c_int) -> c_int,
    create_config: unsafe extern "C" fn(Dpy, c_int, c_int, *mut ConfigAttrib, c_int, *mut u32) -> c_int,
    destroy_config: unsafe extern "C" fn(Dpy, u32) -> c_int,
    create_surfaces: unsafe extern "C" fn(Dpy, u32, u32, u32, *mut u32, u32, *mut c_void, u32) -> c_int,
    destroy_surfaces: unsafe extern "C" fn(Dpy, *mut u32, c_int) -> c_int,
    create_context: unsafe extern "C" fn(Dpy, u32, c_int, c_int, c_int, *mut u32, c_int, *mut u32) -> c_int,
    destroy_context: unsafe extern "C" fn(Dpy, u32) -> c_int,
    create_buffer: unsafe extern "C" fn(Dpy, u32, c_int, u32, u32, *mut c_void, *mut u32) -> c_int,
    destroy_buffer: unsafe extern "C" fn(Dpy, u32) -> c_int,
    begin_picture: unsafe extern "C" fn(Dpy, u32, u32) -> c_int,
    render_picture: unsafe extern "C" fn(Dpy, u32, *mut u32, c_int) -> c_int,
    end_picture: unsafe extern "C" fn(Dpy, u32) -> c_int,
    sync_surface: unsafe extern "C" fn(Dpy, u32) -> c_int,
    max_num_image_formats: unsafe extern "C" fn(Dpy) -> c_int,
    query_image_formats: unsafe extern "C" fn(Dpy, *mut ImageFormat, *mut c_int) -> c_int,
    create_image: unsafe extern "C" fn(Dpy, *mut ImageFormat, c_int, c_int, *mut Image) -> c_int,
    destroy_image: unsafe extern "C" fn(Dpy, u32) -> c_int,
    get_image: unsafe extern "C" fn(Dpy, u32, c_int, c_int, u32, u32, u32) -> c_int,
    map_buffer: unsafe extern "C" fn(Dpy, u32, *mut *mut c_void) -> c_int,
    unmap_buffer: unsafe extern "C" fn(Dpy, u32) -> c_int,
    error_str: unsafe extern "C" fn(c_int) -> *const c_char,
}

fn sym<T: Copy>(lib: *mut c_void, name: &str) -> Result<T, String> {
    let c = CString::new(name).map_err(|e| e.to_string())?;
    // SAFETY: `lib` is a handle from dlopen and `c` is a valid C string.
    let p = unsafe { dlsym(lib, c.as_ptr()) };
    if p.is_null() {
        return Err(format!("libva has no {name}"));
    }
    // SAFETY: the symbol is a function of the type the caller names (a pointer-sized `extern "C"` fn).
    Ok(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
}

fn load(names: &[&str]) -> Result<*mut c_void, String> {
    for n in names {
        let c = CString::new(*n).map_err(|e| e.to_string())?;
        // SAFETY: `c` is a valid C string.
        let h = unsafe { dlopen(c.as_ptr(), RTLD_NOW) };
        if !h.is_null() {
            return Ok(h);
        }
    }
    Err(format!("{} is not installed", names[0]))
}

impl VaApi {
    fn load() -> Result<Self, String> {
        let va = load(&["libva.so.2", "libva.so"])?;
        let drm = load(&["libva-drm.so.2", "libva-drm.so"])?;
        Ok(Self {
            get_display_drm: sym(drm, "vaGetDisplayDRM")?,
            initialize: sym(va, "vaInitialize")?,
            terminate: sym(va, "vaTerminate")?,
            query_config_entrypoints: sym(va, "vaQueryConfigEntrypoints")?,
            max_num_entrypoints: sym(va, "vaMaxNumEntrypoints")?,
            get_config_attributes: sym(va, "vaGetConfigAttributes")?,
            create_config: sym(va, "vaCreateConfig")?,
            destroy_config: sym(va, "vaDestroyConfig")?,
            create_surfaces: sym(va, "vaCreateSurfaces")?,
            destroy_surfaces: sym(va, "vaDestroySurfaces")?,
            create_context: sym(va, "vaCreateContext")?,
            destroy_context: sym(va, "vaDestroyContext")?,
            create_buffer: sym(va, "vaCreateBuffer")?,
            destroy_buffer: sym(va, "vaDestroyBuffer")?,
            begin_picture: sym(va, "vaBeginPicture")?,
            render_picture: sym(va, "vaRenderPicture")?,
            end_picture: sym(va, "vaEndPicture")?,
            sync_surface: sym(va, "vaSyncSurface")?,
            max_num_image_formats: sym(va, "vaMaxNumImageFormats")?,
            query_image_formats: sym(va, "vaQueryImageFormats")?,
            create_image: sym(va, "vaCreateImage")?,
            destroy_image: sym(va, "vaDestroyImage")?,
            get_image: sym(va, "vaGetImage")?,
            map_buffer: sym(va, "vaMapBuffer")?,
            unmap_buffer: sym(va, "vaUnmapBuffer")?,
            error_str: sym(va, "vaErrorStr")?,
        })
    }
}

/// An initialised VA display on a render node.
struct Va {
    api: VaApi,
    dpy: Dpy,
    fd: c_int,
    /// What the driver can decode: HEVC Main and Main 10.
    main: bool,
    main10: bool,
}

impl Va {
    fn open() -> Result<Self, String> {
        let api = VaApi::load()?;
        let mut last = String::from("no GPU render node (/dev/dri/renderD*)");
        for n in 128..136 {
            let path = CString::new(format!("/dev/dri/renderD{n}")).map_err(|e| e.to_string())?;
            // SAFETY: `path` is a valid C string.
            let fd = unsafe { open(path.as_ptr(), O_RDWR | O_CLOEXEC) };
            if fd < 0 {
                continue;
            }
            // SAFETY: `fd` is an open render node.
            let dpy = unsafe { (api.get_display_drm)(fd) };
            if dpy.is_null() {
                // SAFETY: `fd` was opened above.
                unsafe { close(fd) };
                continue;
            }
            let (mut major, mut minor) = (0, 0);
            // SAFETY: `dpy` is a display from vaGetDisplayDRM and the out pointers are valid.
            let st = unsafe { (api.initialize)(dpy, &mut major, &mut minor) };
            if st != VA_STATUS_SUCCESS {
                last = format!("the GPU driver did not start (VA-API status {st})");
                // SAFETY: `fd` was opened above.
                unsafe { close(fd) };
                continue;
            }
            let mut va = Va { api, dpy, fd, main: false, main10: false };
            va.main = va.has_entrypoint(VA_PROFILE_HEVC_MAIN);
            va.main10 = va.has_entrypoint(VA_PROFILE_HEVC_MAIN10);
            return Ok(va);
        }
        Err(last)
    }

    fn has_entrypoint(&self, profile: c_int) -> bool {
        // SAFETY: `dpy` is initialised.
        let max = unsafe { (self.api.max_num_entrypoints)(self.dpy) }.max(1) as usize;
        let mut eps = vec![0 as c_int; max];
        let mut n = 0;
        // SAFETY: `eps` has room for `max` entries, as the call requires.
        let st = unsafe { (self.api.query_config_entrypoints)(self.dpy, profile, eps.as_mut_ptr(), &mut n) };
        st == VA_STATUS_SUCCESS && eps[..n.max(0) as usize].contains(&VA_ENTRYPOINT_VLD)
    }

    fn check(&self, what: &str, st: c_int) -> Result<(), Error> {
        if st == VA_STATUS_SUCCESS {
            return Ok(());
        }
        // SAFETY: vaErrorStr returns a static C string.
        let msg = unsafe { CStr::from_ptr((self.api.error_str)(st)) }.to_string_lossy().into_owned();
        Err(Error::Invalid(format!("VA-API {what} failed: {msg}")))
    }
}

impl Drop for Va {
    fn drop(&mut self) {
        // SAFETY: the display and the descriptor were opened by `open` and nothing uses them any more (every decoder holds an Rc).
        unsafe {
            (self.api.terminate)(self.dpy);
            close(self.fd);
        }
    }
}

/// HEVC decoding through VA-API as a [`PlatformVideo`].
pub struct VaapiPlatform {
    va: RefCell<Option<Result<Rc<Va>, String>>>,
}

impl VaapiPlatform {
    /// Nothing is opened until a stream asks (starting the GPU driver costs a moment).
    pub fn new() -> Self {
        Self { va: RefCell::new(None) }
    }

    fn va(&self) -> Result<Rc<Va>, String> {
        self.va.borrow_mut().get_or_insert_with(|| Va::open().map(Rc::new)).clone()
    }
}

impl Default for VaapiPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformVideo for VaapiPlatform {
    fn name(&self) -> &str {
        "VA-API"
    }

    fn supports(&self, info: &StreamInfo) -> PlatformSupport {
        if info.codec != "hevc" {
            return PlatformSupport::No(format!("VA-API is only used for HEVC here, not {}", info.codec));
        }
        let va = match self.va() {
            Ok(v) => v,
            Err(why) => return PlatformSupport::No(why),
        };
        match rvp_core::hevc_profile(info) {
            Some(1) if va.main => PlatformSupport::Yes,
            Some(2) if va.main10 => PlatformSupport::Yes,
            Some(1) => PlatformSupport::No("this GPU's VA-API driver has no HEVC Main decoder".into()),
            Some(2) => PlatformSupport::No("this GPU's VA-API driver has no HEVC Main 10 decoder".into()),
            Some(_) => PlatformSupport::No("this HEVC profile isn't supported".into()),
            None => PlatformSupport::No("the HEVC stream has no usable configuration".into()),
        }
    }

    fn open(&self, info: &StreamInfo) -> CoreResult<Box<dyn VideoDecoder>> {
        let va = self.va().map_err(|e| Error::Unsupported(format!("video codec `hevc` [VA-API: {e}]")))?;
        let backend = VaBackend { va, state: None, free: Rc::new(RefCell::new(Vec::new())) };
        let stream = HevcStream::new(backend, &info.extra_data)?;
        Ok(Box::new(VaDecoder { stream }))
    }
}

/// The decoder the player drives.
struct VaDecoder {
    stream: HevcStream<VaBackend>,
}

impl VideoDecoder for VaDecoder {
    fn send_packet(&mut self, packet: &Packet) -> CoreResult<()> {
        self.stream.push_sample(&packet.data, packet.pts).map_err(Into::into)
    }

    fn receive_frame(&mut self) -> CoreResult<Option<VideoFrame>> {
        Ok(self.stream.receive())
    }

    fn flush(&mut self) {
        self.stream.flush();
    }

    fn drain(&mut self) -> CoreResult<()> {
        self.stream.drain().map_err(Into::into)
    }
}

/// A surface that returns to its pool when the last reference to the picture goes.
struct SurfaceInner {
    id: u32,
    pool: Rc<RefCell<Vec<u32>>>,
}

impl Drop for SurfaceInner {
    fn drop(&mut self) {
        self.pool.borrow_mut().push(self.id);
    }
}

type Surface = Rc<SurfaceInner>;

/// What exists once the first picture is known: the config, the context, the surfaces and the image to read them with.
struct State {
    config: u32,
    context: u32,
    surfaces: Vec<u32>,
    width: u32,
    height: u32,
    profile: c_int,
    image: Option<(Image, u32)>,
}

struct VaBackend {
    va: Rc<Va>,
    state: Option<State>,
    free: Rc<RefCell<Vec<u32>>>,
}

impl VaBackend {
    fn start(&mut self, sps: &Sps) -> Result<(), Error> {
        let (profile, rt) = if sps.bit_depth_luma == 10 {
            (VA_PROFILE_HEVC_MAIN10, VA_RT_FORMAT_YUV420_10)
        } else {
            (VA_PROFILE_HEVC_MAIN, VA_RT_FORMAT_YUV420)
        };
        if let Some(s) = &self.state {
            if s.width == sps.width && s.height == sps.height && s.profile == profile {
                return Ok(());
            }
            self.stop();
        }
        let va = self.va.clone();
        let mut attr = ConfigAttrib { kind: VA_CONFIG_ATTRIB_RT_FORMAT, value: 0 };
        // SAFETY: valid display, one attribute.
        let st = unsafe { (va.api.get_config_attributes)(va.dpy, profile, VA_ENTRYPOINT_VLD, &mut attr, 1) };
        va.check("vaGetConfigAttributes", st)?;
        if attr.value & rt == 0 {
            return Err(Error::Unsupported(String::from("the GPU cannot decode into this colour format")));
        }
        let mut want = ConfigAttrib { kind: VA_CONFIG_ATTRIB_RT_FORMAT, value: rt };
        let mut config = 0;
        // SAFETY: valid display and attribute.
        let st =
            unsafe { (va.api.create_config)(va.dpy, profile, VA_ENTRYPOINT_VLD, &mut want, 1, &mut config) };
        va.check("vaCreateConfig", st)?;
        let n = sps.dpb_size() + sps.max_num_reorder_pics as usize + 6;
        let mut surfaces = vec![0u32; n];
        // SAFETY: `surfaces` has room for `n` ids.
        let st = unsafe {
            (va.api.create_surfaces)(
                va.dpy,
                rt,
                sps.width,
                sps.height,
                surfaces.as_mut_ptr(),
                n as u32,
                std::ptr::null_mut(),
                0,
            )
        };
        va.check("vaCreateSurfaces", st)?;
        let mut context = 0;
        // SAFETY: `surfaces` holds `n` valid ids.
        let st = unsafe {
            (va.api.create_context)(
                va.dpy,
                config,
                sps.width as c_int,
                sps.height as c_int,
                VA_PROGRESSIVE,
                surfaces.as_mut_ptr(),
                n as c_int,
                &mut context,
            )
        };
        va.check("vaCreateContext", st)?;
        self.free.borrow_mut().clear();
        self.free.borrow_mut().extend(surfaces.iter().copied());
        self.state = Some(State {
            config,
            context,
            surfaces,
            width: sps.width,
            height: sps.height,
            profile,
            image: None,
        });
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(mut s) = self.state.take() {
            let va = &self.va;
            // SAFETY: every id was created by `start` on this display and is destroyed once.
            unsafe {
                if let Some((img, _)) = s.image.take() {
                    (va.api.destroy_image)(va.dpy, img.image_id);
                }
                (va.api.destroy_context)(va.dpy, s.context);
                (va.api.destroy_surfaces)(va.dpy, s.surfaces.as_mut_ptr(), s.surfaces.len() as c_int);
                (va.api.destroy_config)(va.dpy, s.config);
            }
        }
    }

    fn buffer<T>(&self, context: u32, kind: c_int, data: &T, count: u32) -> Result<u32, Error> {
        let mut id = 0;
        // SAFETY: `data` points at `count` elements of the size passed.
        let st = unsafe {
            (self.va.api.create_buffer)(
                self.va.dpy,
                context,
                kind,
                std::mem::size_of::<T>() as u32,
                count,
                data as *const T as *mut c_void,
                &mut id,
            )
        };
        self.va.check("vaCreateBuffer", st)?;
        Ok(id)
    }

    fn raw_buffer(&self, context: u32, kind: c_int, data: &[u8], size_of_element: u32) -> Result<u32, Error> {
        let mut id = 0;
        // SAFETY: `data` holds `len` bytes.
        let st = unsafe {
            (self.va.api.create_buffer)(
                self.va.dpy,
                context,
                kind,
                size_of_element,
                (data.len() as u32) / size_of_element.max(1),
                data.as_ptr() as *mut c_void,
                &mut id,
            )
        };
        self.va.check("vaCreateBuffer", st)?;
        Ok(id)
    }
}

impl Drop for VaBackend {
    fn drop(&mut self) {
        self.stop();
    }
}

fn pic_params(pic: &Picture<'_, Surface>) -> PicParams {
    let (sps, pps, h) = (pic.sps, pic.pps, pic.header);
    let bit = |b: bool, n: u32| (b as u32) << n;
    let no_bi_pred = pic.slices.iter().all(|s| s.header.slice_type != SliceType::B);
    let pic_fields = 1 // chroma_format_idc = 1
        | bit(sps.pcm_enabled, 3)
        | bit(sps.scaling_list_enabled, 4)
        | bit(pps.transform_skip_enabled, 5)
        | bit(sps.amp_enabled, 6)
        | bit(sps.strong_intra_smoothing, 7)
        | bit(pps.sign_data_hiding_enabled, 8)
        | bit(pps.constrained_intra_pred, 9)
        | bit(pps.cu_qp_delta_enabled, 10)
        | bit(pps.weighted_pred, 11)
        | bit(pps.weighted_bipred, 12)
        | bit(pps.transquant_bypass_enabled, 13)
        | bit(pps.tiles_enabled, 14)
        | bit(pps.entropy_coding_sync_enabled, 15)
        | bit(pps.loop_filter_across_slices_enabled, 16)
        | bit(pps.loop_filter_across_tiles_enabled, 17)
        | bit(sps.pcm_loop_filter_disabled, 18)
        | bit(sps.max_num_reorder_pics == 0, 19)
        | bit(no_bi_pred, 20);
    let nal = h.nal;
    let slice_parsing = bit(pps.lists_modification_present, 0)
        | bit(sps.long_term_ref_pics_present, 1)
        | bit(sps.temporal_mvp_enabled, 2)
        | bit(pps.cabac_init_present, 3)
        | bit(pps.output_flag_present, 4)
        | bit(pps.dependent_slice_segments_enabled, 5)
        | bit(pps.slice_chroma_qp_offsets_present, 6)
        | bit(sps.sao_enabled, 7)
        | bit(pps.deblocking_filter_override_enabled, 8)
        | bit(pps.deblocking_filter_disabled, 9)
        | bit(pps.slice_segment_header_extension_present, 10)
        | bit((16..=21).contains(&nal.kind), 11)
        | bit(nal.is_idr(), 12)
        | bit(pic.slices.iter().all(|s| s.header.slice_type == SliceType::I), 13);
    let mut refs = [PictureHevc::INVALID; 15];
    for (i, r) in pic.refs.iter().take(15).enumerate() {
        let mut flags = 0;
        if r.long_term {
            flags |= PIC_LONG_TERM;
        }
        if pic.rps[0].contains(&i) {
            flags |= PIC_ST_CURR_BEFORE;
        }
        if pic.rps[1].contains(&i) {
            flags |= PIC_ST_CURR_AFTER;
        }
        if pic.rps[2].contains(&i) {
            flags |= PIC_LT_CURR;
        }
        refs[i] = PictureHevc { picture_id: r.surface.id, pic_order_cnt: r.poc, flags, reserved: [0; 4] };
    }
    let (cols, rows) = pps.tile_grid(sps);
    let mut column_width_minus1 = [0u16; 19];
    let mut row_height_minus1 = [0u16; 21];
    for (i, w) in cols.iter().take(19).enumerate() {
        column_width_minus1[i] = (*w).saturating_sub(1) as u16;
    }
    for (i, r) in rows.iter().take(21).enumerate() {
        row_height_minus1[i] = (*r).saturating_sub(1) as u16;
    }
    PicParams {
        curr_pic: PictureHevc {
            picture_id: pic.surface.id,
            pic_order_cnt: pic.poc,
            flags: 0,
            reserved: [0; 4],
        },
        reference_frames: refs,
        pic_width_in_luma_samples: sps.width as u16,
        pic_height_in_luma_samples: sps.height as u16,
        pic_fields,
        sps_max_dec_pic_buffering_minus1: sps.max_dec_pic_buffering_minus1,
        bit_depth_luma_minus8: sps.bit_depth_luma - 8,
        bit_depth_chroma_minus8: sps.bit_depth_chroma - 8,
        pcm_sample_bit_depth_luma_minus1: sps.pcm_bit_depth_luma.saturating_sub(1),
        pcm_sample_bit_depth_chroma_minus1: sps.pcm_bit_depth_chroma.saturating_sub(1),
        log2_min_luma_coding_block_size_minus3: sps.log2_min_cb - 3,
        log2_diff_max_min_luma_coding_block_size: sps.log2_ctb - sps.log2_min_cb,
        log2_min_transform_block_size_minus2: sps.log2_min_tb - 2,
        log2_diff_max_min_transform_block_size: sps.log2_max_tb - sps.log2_min_tb,
        log2_min_pcm_luma_coding_block_size_minus3: sps.log2_min_pcm_cb.saturating_sub(3),
        log2_diff_max_min_pcm_luma_coding_block_size: sps.log2_max_pcm_cb.saturating_sub(sps.log2_min_pcm_cb),
        max_transform_hierarchy_depth_intra: sps.max_th_depth_intra,
        max_transform_hierarchy_depth_inter: sps.max_th_depth_inter,
        init_qp_minus26: pps.init_qp_minus26,
        diff_cu_qp_delta_depth: pps.diff_cu_qp_delta_depth,
        pps_cb_qp_offset: pps.cb_qp_offset,
        pps_cr_qp_offset: pps.cr_qp_offset,
        log2_parallel_merge_level_minus2: pps.log2_parallel_merge_level_minus2,
        num_tile_columns_minus1: pps.num_tile_columns_minus1,
        num_tile_rows_minus1: pps.num_tile_rows_minus1,
        column_width_minus1,
        row_height_minus1,
        slice_parsing_fields: slice_parsing,
        log2_max_pic_order_cnt_lsb_minus4: (sps.max_poc_lsb.trailing_zeros() - 4) as u8,
        num_short_term_ref_pic_sets: sps.st_rps.len() as u8,
        num_long_term_ref_pic_sps: sps.lt_ref_pics.len() as u8,
        num_ref_idx_l0_default_active_minus1: pps.num_ref_idx_l0_default_active_minus1,
        num_ref_idx_l1_default_active_minus1: pps.num_ref_idx_l1_default_active_minus1,
        pps_beta_offset_div2: pps.beta_offset_div2,
        pps_tc_offset_div2: pps.tc_offset_div2,
        num_extra_slice_header_bits: pps.num_extra_slice_header_bits,
        st_rps_bits: h.st_rps_bits,
        reserved: [0; 8],
    }
}

/// The up-right diagonal scan of a square block (6.5.3), as (x, y) in coding order.
fn diag_scan(size: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(size * size);
    let (mut x, mut y) = (0i32, 0i32);
    while out.len() < size * size {
        while y >= 0 {
            if (x as usize) < size && (y as usize) < size {
                out.push((x as usize, y as usize));
            }
            y -= 1;
            x += 1;
        }
        y = x;
        x = 0;
    }
    out
}

fn iq_matrix(l: &ScalingList) -> IqMatrix {
    let s4 = diag_scan(4);
    let s8 = diag_scan(8);
    let mut m = IqMatrix {
        scaling_list_4x4: [[0; 16]; 6],
        scaling_list_8x8: [[0; 64]; 6],
        scaling_list_16x16: [[0; 64]; 6],
        scaling_list_32x32: [[0; 64]; 2],
        scaling_list_dc_16x16: l.dc16,
        scaling_list_dc_32x32: l.dc32,
        reserved: [0; 4],
    };
    for id in 0..6 {
        for (i, (x, y)) in s4.iter().enumerate() {
            m.scaling_list_4x4[id][y * 4 + x] = l.l4[id][i];
        }
        for (i, (x, y)) in s8.iter().enumerate() {
            m.scaling_list_8x8[id][y * 8 + x] = l.l8[id][i];
            m.scaling_list_16x16[id][y * 8 + x] = l.l16[id][i];
        }
    }
    for id in 0..2 {
        for (i, (x, y)) in s8.iter().enumerate() {
            m.scaling_list_32x32[id][y * 8 + x] = l.l32[id][i];
        }
    }
    m
}

fn slice_params(pic: &Picture<'_, Surface>, i: usize, offset: u32) -> SliceParams {
    let s = &pic.slices[i];
    let h = &s.header;
    let bit = |b: bool, n: u32| (b as u32) << n;
    let mut p = SliceParams {
        slice_data_size: s.unit.len() as u32,
        slice_data_offset: offset,
        slice_data_flag: 0,
        slice_data_byte_offset: unescaped_offset(s.unit, h.data_offset) as u32,
        slice_segment_address: h.segment_address,
        ref_pic_list: [[0xff; 15]; 2],
        long_slice_flags: bit(i + 1 == pic.slices.len(), 0)
            | bit(h.dependent, 1)
            | ((h.slice_type as u32) << 2)
            | ((h.colour_plane_id as u32) << 4)
            | bit(h.sao_luma, 6)
            | bit(h.sao_chroma, 7)
            | bit(h.mvd_l1_zero, 8)
            | bit(h.cabac_init, 9)
            | bit(h.temporal_mvp, 10)
            | bit(h.deblocking_disabled, 11)
            | bit(h.collocated_from_l0, 12)
            | bit(h.loop_filter_across_slices, 13),
        collocated_ref_idx: h.collocated_ref_idx,
        num_ref_idx_l0_active_minus1: h.num_ref_idx[0].saturating_sub(1),
        num_ref_idx_l1_active_minus1: h.num_ref_idx[1].saturating_sub(1),
        slice_qp_delta: h.qp_delta,
        slice_cb_qp_offset: h.cb_qp_offset,
        slice_cr_qp_offset: h.cr_qp_offset,
        slice_beta_offset_div2: h.beta_offset_div2,
        slice_tc_offset_div2: h.tc_offset_div2,
        luma_log2_weight_denom: 0,
        delta_chroma_log2_weight_denom: 0,
        delta_luma_weight_l0: [0; 15],
        luma_offset_l0: [0; 15],
        delta_chroma_weight_l0: [[0; 2]; 15],
        chroma_offset_l0: [[0; 2]; 15],
        delta_luma_weight_l1: [0; 15],
        luma_offset_l1: [0; 15],
        delta_chroma_weight_l1: [[0; 2]; 15],
        chroma_offset_l1: [[0; 2]; 15],
        five_minus_max_num_merge_cand: h.five_minus_max_num_merge_cand,
        num_entry_point_offsets: h.entry_points.len().min(u16::MAX as usize) as u16,
        entry_offset_to_subset_array: 0,
        slice_data_num_emu_prevn_bytes: h.header_emulation_bytes as u16,
        reserved: [0; 2],
    };
    for l in 0..2 {
        for (k, r) in s.ref_lists[l].iter().take(15).enumerate() {
            p.ref_pic_list[l][k] = *r as u8;
        }
    }
    if let Some(w) = &h.pred_weights {
        p.luma_log2_weight_denom = w.luma_log2_denom;
        p.delta_chroma_log2_weight_denom = w.chroma_log2_denom as i8 - w.luma_log2_denom as i8;
        for (k, e) in w.weights[0].iter().take(15).enumerate() {
            p.delta_luma_weight_l0[k] = (e.luma_weight - (1 << w.luma_log2_denom)) as i8;
            p.luma_offset_l0[k] = e.luma_offset as i8;
            for c in 0..2 {
                p.delta_chroma_weight_l0[k][c] = (e.chroma_weight[c] - (1 << w.chroma_log2_denom)) as i8;
                p.chroma_offset_l0[k][c] = e.chroma_offset[c] as i8;
            }
        }
        for (k, e) in w.weights[1].iter().take(15).enumerate() {
            p.delta_luma_weight_l1[k] = (e.luma_weight - (1 << w.luma_log2_denom)) as i8;
            p.luma_offset_l1[k] = e.luma_offset as i8;
            for c in 0..2 {
                p.delta_chroma_weight_l1[k][c] = (e.chroma_weight[c] - (1 << w.chroma_log2_denom)) as i8;
                p.chroma_offset_l1[k][c] = e.chroma_offset[c] as i8;
            }
        }
    }
    p
}

/// The header end as an offset into the unescaped unit: `data_offset` counts escaped bytes, so take the emulation prevention bytes before
/// it away again.
fn unescaped_offset(unit: &[u8], escaped: usize) -> usize {
    let mut zeros = 0;
    let mut removed = 0;
    for (i, &b) in unit.iter().enumerate() {
        if i >= escaped {
            break;
        }
        if zeros >= 2 && b == 3 {
            removed += 1;
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    escaped - removed
}

impl Backend for VaBackend {
    type Surface = Surface;

    fn alloc(&mut self, sps: &Sps) -> rvp_codec_hevc::Result<Surface> {
        self.start(sps).map_err(|e| match e {
            Error::Unsupported(_) => rvp_codec_hevc::Error::Unsupported("the GPU cannot decode this stream"),
            _ => rvp_codec_hevc::Error::Invalid("the GPU decoder could not be set up"),
        })?;
        let id =
            self.free.borrow_mut().pop().ok_or(rvp_codec_hevc::Error::Invalid("out of decoder surfaces"))?;
        Ok(Rc::new(SurfaceInner { id, pool: self.free.clone() }))
    }

    fn decode(&mut self, pic: &Picture<'_, Surface>) -> rvp_codec_hevc::Result<()> {
        self.decode_inner(pic)
            .map_err(|_| rvp_codec_hevc::Error::Invalid("the GPU decoder rejected a picture"))
    }

    fn read(&mut self, surface: &Surface, sps: &Sps) -> rvp_codec_hevc::Result<VideoFrame> {
        self.read_inner(surface, sps)
            .map_err(|_| rvp_codec_hevc::Error::Invalid("reading a decoded picture back from the GPU failed"))
    }
}

impl VaBackend {
    fn decode_inner(&mut self, pic: &Picture<'_, Surface>) -> Result<(), Error> {
        let state = self.state.as_ref().ok_or(Error::Invalid(String::from("decoder not started")))?;
        let (ctx, target) = (state.context, pic.surface.id);
        let params = pic_params(pic);
        let mut ids: Vec<u32> = Vec::new();
        let result = (|| -> Result<(), Error> {
            ids.push(self.buffer(ctx, BUF_PICTURE_PARAMETER, &params, 1)?);
            if pic.sps.scaling_list_enabled {
                let lists = pic.pps.scaling_list.as_ref().or(pic.sps.scaling_list.as_ref());
                if let Some(l) = lists {
                    ids.push(self.buffer(ctx, BUF_IQ_MATRIX, &iq_matrix(l), 1)?);
                }
            }
            // All the slices' units in one data buffer, one parameter record each.
            let mut data: Vec<u8> = Vec::new();
            let mut slice_params_v: Vec<SliceParams> = Vec::new();
            for i in 0..pic.slices.len() {
                slice_params_v.push(slice_params(pic, i, data.len() as u32));
                data.extend_from_slice(pic.slices[i].unit);
            }
            // SAFETY: `slice_params_v` is a contiguous array of `len` SliceParams.
            let sp_bytes = unsafe {
                std::slice::from_raw_parts(
                    slice_params_v.as_ptr() as *const u8,
                    slice_params_v.len() * std::mem::size_of::<SliceParams>(),
                )
            };
            ids.push(self.raw_buffer(
                ctx,
                BUF_SLICE_PARAMETER,
                sp_bytes,
                std::mem::size_of::<SliceParams>() as u32,
            )?);
            ids.push(self.raw_buffer(ctx, BUF_SLICE_DATA, &data, data.len() as u32)?);
            let va = &self.va;
            // SAFETY: the buffers were created on this context just now.
            unsafe {
                va.check("vaBeginPicture", (va.api.begin_picture)(va.dpy, ctx, target))?;
                let st = (va.api.render_picture)(va.dpy, ctx, ids.as_mut_ptr(), ids.len() as c_int);
                let end = (va.api.end_picture)(va.dpy, ctx);
                va.check("vaRenderPicture", st)?;
                va.check("vaEndPicture", end)?;
            }
            Ok(())
        })();
        for id in ids {
            // SAFETY: created above on this display.
            unsafe { (self.va.api.destroy_buffer)(self.va.dpy, id) };
        }
        result
    }

    fn read_inner(&mut self, surface: &Surface, sps: &Sps) -> Result<VideoFrame, Error> {
        let va = self.va.clone();
        let state = self.state.as_mut().ok_or(Error::Invalid(String::from("decoder not started")))?;
        let ten = sps.bit_depth_luma == 10;
        let fourcc = if ten { FOURCC_P010 } else { FOURCC_NV12 };
        // SAFETY: valid display and surface.
        va.check("vaSyncSurface", unsafe { (va.api.sync_surface)(va.dpy, surface.id) })?;
        if state.image.is_none() {
            // The image format the driver offers for this layout, then an image of the coded size to read surfaces into.
            let max = unsafe { (va.api.max_num_image_formats)(va.dpy) }.max(1) as usize;
            let mut formats = vec![ImageFormat::default(); max];
            let mut n = 0;
            // SAFETY: room for `max` formats.
            va.check("vaQueryImageFormats", unsafe {
                (va.api.query_image_formats)(va.dpy, formats.as_mut_ptr(), &mut n)
            })?;
            let mut fmt = *formats[..n.max(0) as usize].iter().find(|f| f.fourcc == fourcc).ok_or(
                Error::Unsupported(String::from("the GPU has no readable picture format for this stream")),
            )?;
            let mut img = Image::default();
            // SAFETY: valid display, format and out pointer.
            va.check("vaCreateImage", unsafe {
                (va.api.create_image)(va.dpy, &mut fmt, state.width as c_int, state.height as c_int, &mut img)
            })?;
            state.image = Some((img, fourcc));
        }
        let (img, _) = state.image.as_ref().copied().ok_or(Error::Invalid(String::from("no image")))?;
        // SAFETY: valid display, surface and image of the surface's size.
        va.check("vaGetImage", unsafe {
            (va.api.get_image)(va.dpy, surface.id, 0, 0, state.width, state.height, img.image_id)
        })?;
        let mut ptr: *mut c_void = std::ptr::null_mut();
        // SAFETY: the image's buffer id is valid.
        va.check("vaMapBuffer", unsafe { (va.api.map_buffer)(va.dpy, img.buf, &mut ptr) })?;
        let frame = {
            // SAFETY: the mapped buffer is `data_size` bytes long until it is unmapped.
            let data = unsafe { std::slice::from_raw_parts(ptr as *const u8, img.data_size as usize) };
            convert(data, &img, sps, ten)
        };
        // SAFETY: mapped above.
        unsafe { (va.api.unmap_buffer)(va.dpy, img.buf) };
        frame
    }
}

/// NV12 or P010 to a frame, cropped to the conformance window.
fn convert(data: &[u8], img: &Image, sps: &Sps, ten: bool) -> Result<VideoFrame, Error> {
    let (w, h) = sps.display_size();
    crate::semi_planar_frame(
        data,
        SemiPlanar {
            width: (w & !1) as usize,
            height: (h & !1) as usize,
            crop: (sps.crop[0] as usize, sps.crop[2] as usize),
            y: (img.offsets[0] as usize, img.pitches[0] as usize),
            uv: (img.offsets[1] as usize, img.pitches[1] as usize),
            ten,
            colour: sps.colour,
        },
    )
}
