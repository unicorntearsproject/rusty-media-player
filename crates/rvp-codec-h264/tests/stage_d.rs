//! Stage 6d: CABAC (Main profile) must be bit-exact with ffmpeg.
mod common;

macro_rules! fixtures {
    ($($name:ident => $file:expr),* $(,)?) => {
        $(#[test] fn $name() { common::check($file); })*
    };
}

fixtures! {
    c_main_i => "c_main_i.mp4",
    c_main_p => "c_main_p.mp4",
    c_main_b => "c_main_b.mp4",
    c_main_temporal => "c_main_temporal.mp4",
    c_main_pyramid => "c_main_pyramid.mp4",
    c_main_slices => "c_main_slices.mp4",
    c_main_cip => "c_main_cip.mp4",
    c_main_odd => "c_main_odd.mp4",
    c_main_q8 => "c_main_q8.mp4",
    c_main_q48 => "c_main_q48.mp4",
    c_main_1080p => "c_main_1080p.mp4",
}
