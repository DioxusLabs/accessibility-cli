fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    println!("cargo:rerun-if-changed=src/macos/blocks.m");
    cc::Build::new()
        .file("src/macos/blocks.m")
        .flag("-fblocks")
        .flag("-fno-objc-arc")
        .compile("accessibility_ios_blocks");
}
