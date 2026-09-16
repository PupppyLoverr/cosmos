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
    let version = env!("CARGO_PKG_VERSION");
    let file_version = {
        let mut parts = version.split('.').map(|p| p.parse::<u16>().unwrap_or(0));
        format!(
            "{},{},{},0",
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0)
        )
    };
    let ico_path = ico.display().to_string().replace('\\', "\\\\");
    std::fs::write(
        &rc,
        format!(
            "1 ICON \"{ico_path}\"\r\n\
             1 VERSIONINFO\r\n\
             FILEVERSION {file_version}\r\n\
             PRODUCTVERSION {file_version}\r\n\
             FILEFLAGSMASK 0x3fL\r\n\
             FILEFLAGS 0x0L\r\n\
             FILEOS 0x40004L\r\n\
             FILETYPE 0x1L\r\n\
             FILESUBTYPE 0x0L\r\n\
             BEGIN\r\n\
             BLOCK \"StringFileInfo\"\r\n\
             BEGIN\r\n\
             BLOCK \"040904B0\"\r\n\
             BEGIN\r\n\
             VALUE \"CompanyName\", \"Cosmos\"\r\n\
             VALUE \"FileDescription\", \"Cosmos\"\r\n\
             VALUE \"FileVersion\", \"{version}\"\r\n\
             VALUE \"InternalName\", \"zeron\"\r\n\
             VALUE \"OriginalFilename\", \"zeron.exe\"\r\n\
             VALUE \"ProductName\", \"Cosmos\"\r\n\
             VALUE \"ProductVersion\", \"{version}\"\r\n\
             END\r\n\
             END\r\n\
             BLOCK \"VarFileInfo\"\r\n\
             BEGIN\r\n\
             VALUE \"Translation\", 0x409, 1200\r\n\
             END\r\n\
             END\r\n"
        ),
    )
    .unwrap();
    println!("cargo:rerun-if-changed={}", ico.display());
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_required()
        .expect("failed to embed zeron.ico — is the Windows SDK (rc.exe) installed?");
}
