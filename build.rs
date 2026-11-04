//! Embeds `assets/icon.ico` as the exe's icon resource (Windows only). Explorer, shortcuts, the taskbar and
//! Alt+Tab read it from the exe; `window_style::apply` also hands the same resource to the window at run time.
//! A different platform, or a missing resource compiler, must not break the build, so failures only warn.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico"); // resource id 1, the one `window_style` loads
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed the app icon: {e}");
    }
}
