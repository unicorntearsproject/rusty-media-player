//! Stage 6e: High profile (8x8 transform, 8x8 intra prediction, scaling matrices) must be bit-exact with ffmpeg.
mod common;

macro_rules! fixtures {
    ($($name:ident => $file:expr),* $(,)?) => {
        $(#[test] fn $name() { common::check($file); })*
    };
}

fixtures! {
    h_high_i => "h_high_i.mp4",
    h_high => "h_high.mp4",
    h_high_cavlc => "h_high_cavlc.mp4",
    h_high_cqm_jvt => "h_high_cqm_jvt.mp4",
    h_high_cqm_flat => "h_high_cqm_flat.mp4",
    h_high_cqm_cavlc => "h_high_cqm_cavlc.mp4",
    h_high_slices => "h_high_slices.mp4",
    h_high_odd => "h_high_odd.mp4",
    h_high_deblock => "h_high_deblock.mp4",
    h_high_nodb => "h_high_nodb.mp4",
    h_high_refs16 => "h_high_refs16.mp4",
    h_high_mbtree => "h_high_mbtree.mp4",
    h_high_cip => "h_high_cip.mp4",
    h_high_720p => "h_high_720p.mp4",
    h_high_1080p => "h_high_1080p.mp4",
}
