//! Stage 6c: B slices, direct modes and weighted prediction (Main profile, CAVLC) must be bit-exact with ffmpeg.
mod common;

macro_rules! fixtures {
    ($($name:ident => $file:expr),* $(,)?) => {
        $(#[test] fn $name() { common::check($file); })*
    };
}

fixtures! {
    b_cavlc_spatial => "b_cavlc_spatial.mp4",
    b_cavlc_temporal => "b_cavlc_temporal.mp4",
    b_cavlc_pyramid => "b_cavlc_pyramid.mp4",
    b_cavlc_auto => "b_cavlc_auto.mp4",
    b_cavlc_slices => "b_cavlc_slices.mp4",
    b_cavlc_odd => "b_cavlc_odd.mp4",
}
