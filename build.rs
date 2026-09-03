//! Build script.
//!
//! On Windows targets the application icon (assets/icon.ico) is embedded
//! into the .exe resources so Explorer / the taskbar / the window chrome
//! all show the Basalt mark. Other platforms: no-op (macOS icons live in
//! the .app bundle, Linux in the desktop file, neither shipped in-binary).

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        if let Err(e) = winresource::WindowsResource::new()
            .set_icon("assets/icon.ico")
            .set_language(0x0409) // English (US)
            .compile()
        {
            println!("cargo:warning=failed to embed windows icon: {e}");
        }
    }
}
