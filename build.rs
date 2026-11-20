// Embeds the app icon and version info into dart.exe (Windows/MSVC builds).
fn main() {
    println!("cargo:rerun-if-changed=assets/dart.ico");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os == "windows" && env == "msvc" {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/dart.ico") // resource ID 1, loaded by the UI
            .set("FileDescription", "Dart launcher")
            .set("ProductName", "Dart");
        res.compile().expect("failed to embed Windows resources");
    }
}
