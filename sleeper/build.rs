fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set("FileDescription", "chis-quest");
        res.set("ProductName", "chis-quest");
        res.set("InternalName", "chis-quest");
        res.set("OriginalFilename", "chis-quest.exe");
        if let Err(e) = res.compile() {
            println!("cargo:warning=version info not embedded: {e}");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}