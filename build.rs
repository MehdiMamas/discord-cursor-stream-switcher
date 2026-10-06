fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=assets/paused.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon_with_id("assets/app.ico", "1")
        .set_icon_with_id("assets/paused.ico", "2")
        .set_manifest_file("assets/app.manifest")
        .set("ProductName", "Cursor Stream Switcher")
        .set("FileDescription", "Cursor Stream Switcher")
        .set("LegalCopyright", "MIT License");
    res.compile().expect("failed to compile Windows resources");
}
