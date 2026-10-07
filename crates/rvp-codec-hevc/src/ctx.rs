//! Context-coded syntax elements: their positions in the context array and their initial values (9.3.2.2, Tables 9-5 to 9-37), for the
//! three initialisation types (0: I slices; 1 and 2: P and B slices, swapped by `cabac_init_flag`). The initial values are normative data;
//! this layout is ours: one flat array per type, the elements in the order below.

/// First context of `sao_merge` (1).
pub const SAO_MERGE: usize = 0;
/// First context of `sao_type` (1).
pub const SAO_TYPE: usize = 1;
/// First context of `split_cu` (3).
pub const SPLIT_CU: usize = 2;
/// First context of `transquant_bypass` (1).
pub const TRANSQUANT_BYPASS: usize = 5;
/// First context of `skip` (3).
pub const SKIP: usize = 6;
/// First context of `cu_qp_delta` (2).
pub const CU_QP_DELTA: usize = 9;
/// First context of `pred_mode` (1).
pub const PRED_MODE: usize = 12;
/// First context of `part_mode` (4).
pub const PART_MODE: usize = 13;
/// First context of `prev_intra_luma` (1).
pub const PREV_INTRA_LUMA: usize = 17;
/// First context of `intra_chroma` (1).
pub const INTRA_CHROMA: usize = 18;
/// First context of `merge_flag` (1).
pub const MERGE_FLAG: usize = 20;
/// First context of `merge_idx` (1).
pub const MERGE_IDX: usize = 21;
/// First context of `inter_pred_idc` (5).
pub const INTER_PRED_IDC: usize = 22;
/// First context of `ref_idx` (2).
pub const REF_IDX: usize = 27;
/// First context of `mvd_greater0` (1).
pub const MVD_GREATER0: usize = 31;
/// First context of `mvd_greater1` (1).
pub const MVD_GREATER1: usize = 32;
/// First context of `mvp_flag` (1).
pub const MVP_FLAG: usize = 35;
/// First context of `rqt_root_cbf` (1).
pub const RQT_ROOT_CBF: usize = 36;
/// First context of `split_transform` (3).
pub const SPLIT_TRANSFORM: usize = 37;
/// First context of `cbf_luma` (2).
pub const CBF_LUMA: usize = 40;
/// First context of `cbf_chroma` (4).
pub const CBF_CHROMA: usize = 42;
/// First context of `transform_skip` (2).
pub const TRANSFORM_SKIP: usize = 47;
/// First context of `last_x_prefix` (18).
pub const LAST_X_PREFIX: usize = 53;
/// First context of `last_y_prefix` (18).
pub const LAST_Y_PREFIX: usize = 71;
/// First context of `coded_sub_block` (4).
pub const CODED_SUB_BLOCK: usize = 89;
/// First context of `sig_coeff` (42).
pub const SIG_COEFF: usize = 93;
/// First context of `greater1` (24).
pub const GREATER1: usize = 137;
/// First context of `greater2` (6).
pub const GREATER2: usize = 161;
/// Contexts in all.
pub const COUNT: usize = 167;

/// Initial values by initialisation type.
pub const INIT_VALUES: [[u8; COUNT]; 3] = [
    [
        153, 200, 139, 141, 157, 154, 154, 154, 154, 154, 154, 154, 154, 184, 154, 154, 154, 184, 63, 139,
        154, 154, 154, 154, 154, 154, 154, 154, 154, 154, 154, 154, 154, 154, 154, 154, 154, 153, 138, 138,
        111, 141, 94, 138, 182, 154, 154, 139, 139, 139, 139, 139, 139, 110, 110, 124, 125, 140, 153, 125,
        127, 140, 109, 111, 143, 127, 111, 79, 108, 123, 63, 110, 110, 124, 125, 140, 153, 125, 127, 140,
        109, 111, 143, 127, 111, 79, 108, 123, 63, 91, 171, 134, 141, 111, 111, 125, 110, 110, 94, 124, 108,
        124, 107, 125, 141, 179, 153, 125, 107, 125, 141, 179, 153, 125, 107, 125, 141, 179, 153, 125, 140,
        139, 182, 182, 152, 136, 152, 136, 153, 136, 139, 111, 136, 139, 111, 141, 111, 140, 92, 137, 138,
        140, 152, 138, 139, 153, 74, 149, 92, 139, 107, 122, 152, 140, 179, 166, 182, 140, 227, 122, 197,
        138, 153, 136, 167, 152, 152,
    ],
    [
        153, 185, 107, 139, 126, 154, 197, 185, 201, 154, 154, 154, 149, 154, 139, 154, 154, 154, 152, 139,
        110, 122, 95, 79, 63, 31, 31, 153, 153, 153, 153, 140, 198, 140, 198, 168, 79, 124, 138, 94, 153,
        111, 149, 107, 167, 154, 154, 139, 139, 139, 139, 139, 139, 125, 110, 94, 110, 95, 79, 125, 111, 110,
        78, 110, 111, 111, 95, 94, 108, 123, 108, 125, 110, 94, 110, 95, 79, 125, 111, 110, 78, 110, 111,
        111, 95, 94, 108, 123, 108, 121, 140, 61, 154, 155, 154, 139, 153, 139, 123, 123, 63, 153, 166, 183,
        140, 136, 153, 154, 166, 183, 140, 136, 153, 154, 166, 183, 140, 136, 153, 154, 170, 153, 123, 123,
        107, 121, 107, 121, 167, 151, 183, 140, 151, 183, 140, 140, 140, 154, 196, 196, 167, 154, 152, 167,
        182, 182, 134, 149, 136, 153, 121, 136, 137, 169, 194, 166, 167, 154, 167, 137, 182, 107, 167, 91,
        122, 107, 167,
    ],
    [
        153, 160, 107, 139, 126, 154, 197, 185, 201, 154, 154, 154, 134, 154, 139, 154, 154, 183, 152, 139,
        154, 137, 95, 79, 63, 31, 31, 153, 153, 153, 153, 169, 198, 169, 198, 168, 79, 224, 167, 122, 153,
        111, 149, 92, 167, 154, 154, 139, 139, 139, 139, 139, 139, 125, 110, 124, 110, 95, 94, 125, 111, 111,
        79, 125, 126, 111, 111, 79, 108, 123, 93, 125, 110, 124, 110, 95, 94, 125, 111, 111, 79, 125, 126,
        111, 111, 79, 108, 123, 93, 121, 140, 61, 154, 170, 154, 139, 153, 139, 123, 123, 63, 124, 166, 183,
        140, 136, 153, 154, 166, 183, 140, 136, 153, 154, 166, 183, 140, 136, 153, 154, 170, 153, 138, 138,
        122, 121, 122, 121, 167, 151, 183, 140, 151, 183, 140, 140, 140, 154, 196, 167, 167, 154, 152, 167,
        182, 182, 134, 149, 136, 153, 121, 136, 122, 169, 208, 166, 167, 154, 152, 167, 182, 107, 167, 91,
        107, 107, 167,
    ],
];
