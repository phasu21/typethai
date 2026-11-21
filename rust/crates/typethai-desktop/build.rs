//! Windows: put the app icon (`packaging/typethai.ico`) into typethai-desktop.exe, so Explorer, the taskbar, the Start
//! menu and the installer's uninstall entry show typeThai's icon. Nothing to do on other targets.
fn main() {
    let ico = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packaging/typethai.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") { return }
    let ico = ico.canonicalize().expect("packaging/typethai.ico");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let rc = out.join("typethai.rc");
    // ICON resource 1: the first icon in the exe is the one Windows shows for it
    let path = ico.display().to_string().replace('\\', "\\\\");
    std::fs::write(&rc, format!("1 ICON \"{path}\"\n")).expect("write typethai.rc");
    embed_resource::compile(&rc, embed_resource::NONE).manifest_optional().expect("compile the icon resource");
}
