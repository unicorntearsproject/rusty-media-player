//! Stage 6b: P slices (Baseline) must be bit-exact with ffmpeg.
mod common;

macro_rules! fixtures {
    ($($name:ident => $file:expr),* $(,)?) => {
        $(#[test] fn $name() { common::check($file); })*
    };
}

fixtures! {
    p_base_nodb => "p_base_nodb.mp4",
    p_base_ref1 => "p_base_ref1.mp4",
    p_base_ref4 => "p_base_ref4.mp4",
    p_base_ref16 => "p_base_ref16.mp4",
    p_base_cip => "p_base_cip.mp4",
    p_base_slices => "p_base_slices.mp4",
    p_base_odd => "p_base_odd.mp4",
    p_base_720p => "p_base_720p.mp4",
}
