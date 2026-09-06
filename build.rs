fn main() {
    println!("cargo:rerun-if-changed=assets/ply.ico");
    println!("cargo:rerun-if-changed=assets/ply.rc");
    // EXE icon is Windows-only. GPUI loads resource ID 1 for the
    // taskbar/titlebar/Alt-Tab icon; other targets skip this entirely.
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "windows" {
        return;
    }
    let _ = embed_resource::compile("assets/ply.rc", embed_resource::NONE);
}
