//! Stage 6a: intra-only CAVLC streams must be bit-exact with ffmpeg.
mod common;

macro_rules! fixtures {
    ($($name:ident => $file:expr),* $(,)?) => {
        $(#[test] fn $name() { common::check($file); })*
    };
}

fixtures! {
    i_base_nodb => "i_base_nodb.mp4",
    i_base_cif => "i_base_cif.mp4",
    i_base_odd => "i_base_odd.mp4",
    i_base_slices => "i_base_slices.mp4",
    i_base_q5 => "i_base_q5.mp4",
    i_base_q45 => "i_base_q45.mp4",
    i_base_720p => "i_base_720p.mp4",
}
