fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    println!("cargo:rerun-if-changed=src/macos/blocks.c");
    cc::Build::new()
        .file("src/macos/blocks.c")
        .flag("-fblocks")
        .compile("accessibility_ios_blocks");
}
