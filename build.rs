fn main() {
    #[cfg(feature = "c-reference")]
    build_misa77();
}

#[cfg(feature = "c-reference")]
fn build_misa77() {
    let vendor = "vendor/misa77";

    let mut base = cc::Build::new();
    base.cpp(true)
        .std("c++20")
        .opt_level(3)
        .include(format!("{vendor}/include"))
        .include(format!("{vendor}/src"))
        .warnings(false);

    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    if arch == "x86_64" {
        // Portable TU (no special flags)
        let mut portable = base.clone();
        portable
            .file(format!("{vendor}/src/isa/target_portable.cpp"))
            .compile("misa77_portable");

        // SSE2 TU (baseline on x86-64)
        let mut sse2 = base.clone();
        sse2.flag("-msse2")
            .file(format!("{vendor}/src/isa/target_sse2.cpp"))
            .compile("misa77_sse2");

        // AVX2 TU
        let mut avx2 = base.clone();
        avx2.flag("-mavx2")
            .file(format!("{vendor}/src/isa/target_avx2.cpp"))
            .compile("misa77_avx2");
    } else {
        // ARM64 / other: portable only
        let mut portable = base.clone();
        portable
            .file(format!("{vendor}/src/isa/target_portable.cpp"))
            .compile("misa77_portable");
    }

    // Main dispatch files (no special ISA flags needed)
    let mut main_build = base.clone();
    main_build
        .file(format!("{vendor}/src/compress.cpp"))
        .file(format!("{vendor}/src/decompress.cpp"))
        .file("vendor/misa77_ffi.cpp")
        .compile("misa77_main");

    println!("cargo:rerun-if-changed=vendor/misa77_ffi.cpp");
    println!("cargo:rerun-if-changed={vendor}/src");
    println!("cargo:rerun-if-changed={vendor}/include");
}
