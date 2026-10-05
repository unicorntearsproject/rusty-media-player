//! Build script: on Windows, embeds the icon and the version information in `rvp.exe`.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
}
