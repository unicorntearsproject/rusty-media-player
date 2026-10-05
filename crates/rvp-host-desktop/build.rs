//! Build script: for a Windows target, embeds the icon, the version information and a manifest (per-monitor DPI, UTF-8, long
//! paths) in `rusty-wave.exe`. Nothing happens for other targets.
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=windows/rusty-wave.manifest");
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        return;
    }
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.join("../..");
    let icon = root.join("packaging/icons/rusty-wave.ico");
    let manifest = manifest_dir.join("windows/rusty-wave.manifest");
    println!("cargo:rerun-if-changed={}", icon.display());
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let num = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let (major, minor, patch) =
        (num("CARGO_PKG_VERSION_MAJOR"), num("CARGO_PKG_VERSION_MINOR"), num("CARGO_PKG_VERSION_PATCH"));
    let esc = |p: &std::path::Path| p.to_string_lossy().replace('\\', "/");
    let rc = format!(
        r#"#include <winver.h>
1 ICON "{icon}"
1 24 "{manifest}"
VS_VERSION_INFO VERSIONINFO
 FILEVERSION {major},{minor},{patch},0
 PRODUCTVERSION {major},{minor},{patch},0
 FILEFLAGSMASK 0x3fL
 FILEFLAGS 0x0L
 FILEOS 0x40004L
 FILETYPE 0x1L
 FILESUBTYPE 0x0L
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "Rusty Wave contributors"
      VALUE "FileDescription", "Rusty Wave"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "rusty-wave"
      VALUE "LegalCopyright", "MIT OR Apache-2.0"
      VALUE "OriginalFilename", "rusty-wave.exe"
      VALUE "ProductName", "Rusty Wave"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = esc(&icon),
        manifest = esc(&manifest),
    );
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("rusty-wave.rc");
    std::fs::write(&out, rc).expect("write rusty-wave.rc");
    embed_resource::compile(&out, embed_resource::NONE)
        .manifest_required()
        .expect("embed the Windows resources");
}
