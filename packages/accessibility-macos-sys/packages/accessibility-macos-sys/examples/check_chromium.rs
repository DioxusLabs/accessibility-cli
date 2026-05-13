fn main() {
    let pid: u32 = std::env::args().nth(1).expect("pid").parse().expect("u32 pid");
    let bundle = accessibility_macos_sys::bundle_path_for_pid(pid);
    let is_chromium = accessibility_macos_sys::is_chromium_based_app(pid);
    println!("pid={pid} bundle={bundle:?} is_chromium={is_chromium}");
}
