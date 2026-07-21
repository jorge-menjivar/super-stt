fn main() {
    // Record the build variant so the daemon can report it at runtime (see
    // `cli::BUILD_VARIANT`). CI sets it explicitly (BUILD_VARIANT=cpu); local
    // builds default to "cpu". GPU residency lives in out-of-tree backends, so
    // there are no GPU-specific build variants.
    let build_variant = std::env::var("BUILD_VARIANT").unwrap_or_else(|_| "cpu".to_string());
    println!("cargo:rustc-env=BUILD_VARIANT={build_variant}");
    println!("cargo:warning=Build variant: {build_variant}");

    // For cross-compilation verification
    if let Ok(target) = std::env::var("TARGET") {
        println!("cargo:warning=Building for target: {target}");
    }
}
