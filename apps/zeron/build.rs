//! Windows only: embed `dist/windows/zeron.ico` into zeron.exe as icon
//! resource ID 1. That single resource covers Explorer's file icon, the
//! taskbar/window icon (gpui's `load_icon` issues `LoadImageW(module, 1,
//! IMAGE_ICON)`), installer shortcuts, and the Apps & Features entry.
//! macOS/Linux/iOS icons are packaged at bundle/install time — see
//! dist/README.md and scripts/generate-icons.mjs.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest_dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let ico = manifest_dir
        .join("../../dist/windows/zeron.ico")
        .canonicalize()
        .expect("dist/windows/zeron.ico missing — run scripts/generate-icons.mjs");
    // rc paths need escaped backslashes; emit the .rc into OUT_DIR so the
    // absolute icon path works from any checkout location.
    let rc = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("zeron.rc");
    std::fs::write(
        &rc,
        format!(
            "1 ICON \"{}\"\r\n",
            ico.display().to_string().replace('\\', "\\\\")
        ),
    )
    .unwrap();
    println!("cargo:rerun-if-changed={}", ico.display());
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_required()
        .expect("failed to embed zeron.ico — is the Windows SDK (rc.exe) installed?");
}
