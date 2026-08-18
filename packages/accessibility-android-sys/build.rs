fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    let mut prost = tonic_build::Config::new();
    prost.protoc_executable(protoc);
    tonic_build::configure()
        .build_server(false)
        .compile_protos_with_config(
            prost,
            &[
                "proto/emulator_controller.proto",
                "proto/rtc_service_v2.proto",
            ],
            &["proto"],
        )?;
    Ok(())
}
